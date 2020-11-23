/// Interpreter for Jsonnet.
pub struct JsonnetVm {
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

#[repr(C)]
struct NativeCallbackHolder {
    vm: *mut gojsonnet_sys::JsonnetVm,
    callback: unsafe fn(
        vm: *mut gojsonnet_sys::JsonnetVm,
        argv: *const *const gojsonnet_sys::JsonnetJsonValue,
    ) -> Option<*mut gojsonnet_sys::JsonnetJsonValue>,
}
unsafe extern "C" fn native_callback_bridge(
    ctx: *mut std::ffi::c_void,
    argv: *const *const gojsonnet_sys::JsonnetJsonValue,
    success: *mut i32,
) -> *mut gojsonnet_sys::JsonnetJsonValue {
    let holder = ctx as *mut NativeCallbackHolder;
    let vm = (*holder).vm;
    let callback = (*holder).callback;
    if let Some(result) = callback(vm, argv) {
        *success = 1;
        result
    } else {
        gojsonnet_sys::jsonnet_json_make_null(vm)
    }
}

impl JsonnetVm {
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            inner: unsafe { gojsonnet_sys::jsonnet_make() },
        }
    }

    /// Return the version of underlying google/go-jsonnet library.
    pub fn library_version() -> String {
        let version_cstr = unsafe { std::ffi::CStr::from_ptr(gojsonnet_sys::jsonnet_version()) };
        version_cstr.to_str().unwrap().to_owned()
    }

    /// Evaluate a Jsonnet code and return a JSON string.
    pub fn evaluate_snippet(&self, filename: &str, code: &str) -> Result<String, Error> {
        let filename_ptr = std::ffi::CString::new(filename)?.into_raw();
        let code_ptr = std::ffi::CString::new(code)?.into_raw();
        let mut err = 0;
        let result_cstr = unsafe {
            std::ffi::CStr::from_ptr(gojsonnet_sys::jsonnet_evaluate_snippet(
                self.inner,
                filename_ptr,
                code_ptr,
                &mut err,
            ))
        };
        let result = result_cstr.to_str().unwrap().to_owned();
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
        callback: unsafe fn(
            vm: *mut gojsonnet_sys::JsonnetVm,
            argv: *const *const gojsonnet_sys::JsonnetJsonValue,
        ) -> Option<*mut gojsonnet_sys::JsonnetJsonValue>,
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
}
impl Drop for JsonnetVm {
    fn drop(&mut self) {
        unsafe { gojsonnet_sys::jsonnet_destroy(self.inner) };
    }
}

impl Default for JsonnetVm {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert!(!super::JsonnetVm::library_version().is_empty());
    }

    #[test]
    fn evaluate_snippet_ok() {
        let vm = super::JsonnetVm::default();
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
        let vm = super::JsonnetVm::default();
        let e = vm
            .evaluate_snippet("evaluate_snippet_syntax_error.jsonnet", "{foo: bar}")
            .unwrap_err();
        assert!(e
            .to_string()
            .starts_with("go-jsonnet returned error: evaluate_snippet_syntax_error.jsonnet:1:"));
        assert!(e.to_string().contains("Unknown variable"));
    }

    #[test]
    fn native_callback_ok() {
        let mut vm = super::JsonnetVm::default();
        vm.native_callback("hello", &["arg1"], |vm, argv| unsafe {
            let arg1_c = gojsonnet_sys::jsonnet_json_extract_string(
                vm,
                *argv as *mut gojsonnet_sys::JsonnetJsonValue,
            );
            let arg1 = std::ffi::CStr::from_ptr(arg1_c).to_str().unwrap();
            let message = gojsonnet_sys::jsonnet_json_make_string(
                vm,
                std::ffi::CString::new(format!("hello {}", arg1))
                    .unwrap()
                    .into_raw(),
            );
            Some(message)
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
}
