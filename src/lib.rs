/// Interpreter for Jsonnet.
pub struct Vm {
    inner: *mut gojsonnet_sys::JsonnetVm,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum Error {
    /// Error returned from Jsonnet interpreter.
    #[error("go-jsonnet returned error: {message}")]
    GoJsonnetError { message: String },
    /// Error while converting Rust string to C string.
    #[error("C-string nul error: {inner}")]
    NulError {
        #[from]
        inner: std::ffi::NulError,
    },
}

pub type NativeCallback = fn(argv: Vec<serde_json::Value>) -> Option<serde_json::Value>;

#[repr(C)]
struct NativeCallbackHolder {
    vm: *mut gojsonnet_sys::JsonnetVm,
    callback: NativeCallback,
    argc: usize,
}
unsafe extern "C" fn native_callback_bridge(
    ctx: *mut std::ffi::c_void,
    argv_c: *const *const gojsonnet_sys::JsonnetJsonValue,
    success: *mut i32,
) -> *mut gojsonnet_sys::JsonnetJsonValue {
    let holder = ctx as *mut NativeCallbackHolder;
    let vm = (*holder).vm;
    let callback = (*holder).callback;
    let argc = (*holder).argc;
    let mut argv = Vec::with_capacity(argc);
    for i in 0..argc {
        argv.push(from_gojsonnet_value(
            vm,
            *argv_c.offset(i as isize) as *mut gojsonnet_sys::JsonnetJsonValue,
        ));
    }
    if let Some(result) = callback(argv) {
        *success = 1;
        from_serde_json_value(vm, result)
    } else {
        gojsonnet_sys::jsonnet_json_make_null(vm)
    }
}

unsafe fn from_serde_json_value(
    vm: *mut gojsonnet_sys::JsonnetVm,
    value: serde_json::Value,
) -> *mut gojsonnet_sys::JsonnetJsonValue {
    match value {
        serde_json::Value::Null => gojsonnet_sys::jsonnet_json_make_null(vm),
        serde_json::Value::Bool(b) => gojsonnet_sys::jsonnet_json_make_bool(vm, b.into()),
        serde_json::Value::Number(n) => {
            gojsonnet_sys::jsonnet_json_make_number(vm, n.as_f64().unwrap())
        }
        serde_json::Value::String(s) => gojsonnet_sys::jsonnet_json_make_string(
            vm,
            std::ffi::CString::new(s).unwrap().into_raw(),
        ),
        serde_json::Value::Array(v) => {
            let ary = gojsonnet_sys::jsonnet_json_make_array(vm);
            for e in v {
                gojsonnet_sys::jsonnet_json_array_append(vm, ary, from_serde_json_value(vm, e));
            }
            ary
        }
        serde_json::Value::Object(m) => {
            let obj = gojsonnet_sys::jsonnet_json_make_object(vm);
            for (k, v) in m {
                gojsonnet_sys::jsonnet_json_object_append(
                    vm,
                    obj,
                    std::ffi::CString::new(k).unwrap().into_raw(),
                    from_serde_json_value(vm, v),
                );
            }
            obj
        }
    }
}

unsafe fn from_gojsonnet_value(
    vm: *mut gojsonnet_sys::JsonnetVm,
    value: *mut gojsonnet_sys::JsonnetJsonValue,
) -> serde_json::Value {
    if gojsonnet_sys::jsonnet_json_extract_null(vm, value) != 0 {
        return serde_json::Value::Null;
    }
    let b = gojsonnet_sys::jsonnet_json_extract_bool(vm, value);
    if b == 0 {
        return serde_json::Value::Bool(false);
    } else if b == 1 {
        return serde_json::Value::Bool(true);
    }
    let mut n = 0.0;
    if gojsonnet_sys::jsonnet_json_extract_number(vm, value, &mut n) != 0 {
        return serde_json::Value::Number(serde_json::Number::from_f64(n).unwrap());
    }
    let c_str = gojsonnet_sys::jsonnet_json_extract_string(vm, value);
    if !c_str.is_null() {
        let s = std::ffi::CStr::from_ptr(c_str).to_str().unwrap().to_owned();
        return serde_json::Value::String(s);
    }
    // XXX: array and object?

    panic!("Unsupported value: {:?}", value);
}

impl Vm {
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            inner: unsafe { gojsonnet_sys::jsonnet_make() },
        }
    }

    /// Return the version of underlying google/go-jsonnet library.
    pub fn library_version() -> String {
        let version_cstr = unsafe { std::ffi::CStr::from_ptr(gojsonnet_sys::jsonnet_version()) };
        version_cstr.to_string_lossy().into_owned()
    }

    /// Evaluate a Jsonnet code and return a JSON string.
    pub fn evaluate_snippet(&self, filename: &str, code: &str) -> Result<String, Error> {
        let filename_ptr = std::ffi::CString::new(filename)?.into_raw();
        let code_ptr = std::ffi::CString::new(code)?.into_raw();
        let mut err = 0;
        let result = unsafe {
            let ptr = gojsonnet_sys::jsonnet_evaluate_snippet(
                self.inner,
                filename_ptr,
                code_ptr,
                &mut err,
            );
            let s = std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned();
            gojsonnet_sys::jsonnet_realloc(self.inner, ptr, 0);
            s
        };
        if err == 0 {
            Ok(result)
        } else {
            Err(Error::GoJsonnetError { message: result })
        }
    }

    /// Register a native function to the interpreter.
    pub fn native_callback(
        &mut self,
        name: &str,
        params: &[&str],
        callback: NativeCallback,
    ) -> Result<(), Error> {
        let name_ptr = std::ffi::CString::new(name)?.into_raw();
        let mut params_c = Vec::with_capacity(params.len());
        for param in params {
            params_c.push(std::ffi::CString::new(*param)?.into_raw());
        }
        params_c.push(std::ptr::null_mut());
        let holder = Box::into_raw(Box::new(NativeCallbackHolder {
            vm: self.inner,
            callback,
            argc: params.len(),
        }));
        unsafe {
            gojsonnet_sys::jsonnet_native_callback(
                self.inner,
                name_ptr,
                Some(native_callback_bridge),
                holder as *mut std::ffi::c_void,
                params_c.as_mut_ptr(),
            )
        };
        Ok(())
    }

    /// Bind a Jsonnet external variable to the given string.
    pub fn ext_var(&mut self, key: &str, val: &str) -> Result<(), Error> {
        let key_ptr = std::ffi::CString::new(key)?.into_raw();
        let val_ptr = std::ffi::CString::new(val)?.into_raw();
        unsafe { gojsonnet_sys::jsonnet_ext_var(self.inner, key_ptr, val_ptr) };
        Ok(())
    }
}
impl Drop for Vm {
    fn drop(&mut self) {
        unsafe { gojsonnet_sys::jsonnet_destroy(self.inner) };
    }
}

impl Default for Vm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let v = super::Vm::library_version();
        assert!(!v.is_empty(), "v = {:?}, v");
    }

    #[test]
    fn evaluate_snippet_ok() {
        let vm = super::Vm::default();
        let json_str = vm
            .evaluate_snippet(
                "evaluate_snippet_ok.jsonnet",
                "{foo: 1+2, bar: std.isBoolean(false)}",
            )
            .unwrap();
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct S {
            foo: i32,
            bar: bool,
        }
        let s: S = serde_json::from_str(&json_str).unwrap();
        assert_eq!(s, S { foo: 3, bar: true });
    }

    #[test]
    fn evaluate_snippet_syntax_error() {
        let vm = super::Vm::default();
        let e = vm
            .evaluate_snippet("evaluate_snippet_syntax_error.jsonnet", "{foo: bar}")
            .unwrap_err();
        assert!(
            e.to_string()
                .starts_with("go-jsonnet returned error: evaluate_snippet_syntax_error.jsonnet:1:"),
            "e = {}",
            e
        );
        assert!(e.to_string().contains("Unknown variable"), "e = {}", e);
    }

    #[test]
    fn native_callback_ok() {
        let mut vm = super::Vm::default();
        vm.native_callback("hello", &["arg1"], |argv| {
            let arg1 = argv[0].as_str().unwrap();
            Some(serde_json::json!(format!("hello {}", arg1)))
        })
        .unwrap();
        let json_str = vm
            .evaluate_snippet(
                "native_callback_ok.jsonnet",
                r#"local hello = std.native("hello"); {message: hello("world")}"#,
            )
            .unwrap();
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct S {
            message: String,
        }
        let s: S = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            s,
            S {
                message: "hello world".to_owned()
            }
        );
    }

    #[test]
    fn ext_var_ok() {
        let mut vm = super::Vm::default();
        vm.ext_var("appId", "gojsonnet").unwrap();
        let json_str = vm
            .evaluate_snippet("ext_var_ok.jsonnet", "{ e: std.extVar('appId') }")
            .unwrap();
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct S {
            e: String,
        }
        let s: S = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            s,
            S {
                e: "gojsonnet".to_owned()
            }
        );
    }
}
