use std::ffi::{CStr, CString};
use std::ptr::NonNull;
use std::str;

use anyhow::Result;
use clap::Parser;
use mozjs::jsapi::CallArgs;
use mozjs::jsapi::mozilla::Utf8Unit;
use mozjs::jsapi::{
    Handle, JSContext, JSObject, OnNewGlobalHookOption, SetModuleResolveHook,
    JS_ReportErrorASCII,
    SourceText, Value,
};
use mozjs::context::{JSContext as EmbeddedJSContext, RawJSContext};
use mozjs::realm::AutoRealm;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{
    CompileModule1, InitRealmStandardClasses, JS_DefineFunction, JS_NewGlobalObject,
    JS_NewPlainObject, JS_SetProperty, ModuleEvaluate, ModuleLink,
};
use mozjs::rust::{
    CompileOptionsWrapper, JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS,
    ToString, transform_str_to_source_text,
};
use mozjs::rust::wrappers2::EncodeStringToUTF8;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    file: String,
}

unsafe extern "C" fn console_log(context: *mut RawJSContext, argc: u32, vp: *mut Value) -> bool {
    unsafe {
        let mut context = EmbeddedJSContext::from_ptr(NonNull::new(context).unwrap());
        let args = CallArgs::from_vp(vp, argc);

        if args.argc_ != 1 {
            JS_ReportErrorASCII(
                context.raw_cx(),
                c"console.log() requires exactly 1 argument".as_ptr(),
            );
            return false;
        }

        let arg = mozjs::rust::Handle::from_raw(args.get(0));
        let js = ToString(context.raw_cx(), arg);
        rooted!(&in(context) let message_root = js);

        unsafe extern "C" fn write(message: *const core::ffi::c_char) {
            let message = unsafe { CStr::from_ptr(message) };
            let message = str::from_utf8(message.to_bytes()).unwrap();
            println!("{}", message);
        }

        EncodeStringToUTF8(&mut context, message_root.handle().into(), write);

        args.rval().set(UndefinedValue());
        true
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let engine = JSEngine::init().expect("JS Engine initialization failed");
    let mut runtime = Runtime::new(engine.handle());

    unsafe extern "C" fn module_resolve_hook(
        _cx: *mut JSContext,
        _referencing_private: Handle<Value>,
        _specifier: Handle<*mut JSObject>,
    ) -> *mut JSObject {
        std::ptr::null_mut()
    }

    unsafe {
        SetModuleResolveHook(runtime.rt(), Some(module_resolve_hook));
    };

    let options = RealmOptions::default();
    rooted!(&in(runtime.cx()) let global = unsafe {
        JS_NewGlobalObject(
            runtime.cx(),
            &SIMPLE_GLOBAL_CLASS,
            std::ptr::null_mut(),
            OnNewGlobalHookOption::FireOnNewGlobalHook,
            &*options
        )
    });

    let mut realm = AutoRealm::new_from_handle(runtime.cx(), global.handle());
    let (global_handle, realm) = realm.global_and_reborrow();
    let cx: &mut EmbeddedJSContext = &mut *realm;

    unsafe {
        assert!(InitRealmStandardClasses(cx));

        rooted!(&in(cx) let console = JS_NewPlainObject(cx));
        let function = JS_DefineFunction(cx, console.handle().into(), c"log".as_ptr(), Some(console_log), 1, 0);
        assert!(!function.is_null());
        rooted!(&in(cx) let console_value = ObjectValue(console.get()));
        assert!(JS_SetProperty(cx, global_handle, c"console".as_ptr(), console_value.handle()));
    }

    let options =
        CompileOptionsWrapper::new(cx, CString::new(args.file.clone()).unwrap(), 1);
    let source = tokio::fs::read_to_string(&args.file).await?;
    let mut source_text = transform_str_to_source_text(&source);
    let source: *mut SourceText<Utf8Unit> = &mut source_text;
    unsafe {
        rooted!(&in(cx) let module = CompileModule1(cx, options.ptr, source));
        ModuleLink(cx, module.handle());
        rooted!(&in(cx) let mut rval = UndefinedValue());
        ModuleEvaluate(cx, module.handle(), rval.handle_mut());
    }

    println!("Processing file: {}", args.file);
    Ok(())
}
