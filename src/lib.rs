/// Interpreter for Jsonnet.
pub struct Vm {
    inner: *mut gojsonnet_sys::JsonnetVm,
    native_callback_holders: std::collections::HashMap<String, *mut NativeCallbackHolder>,
    import_callback_holder: Option<*mut ImportCallbackHolder>,
}

#[derive(Debug, thiserror::Error)]
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
    /// Error while deserializing JSON returned from jsonnet_evaluate_snippet API.
    #[error("Serde error: {inner}")]
    SerdeError {
        #[from]
        inner: serde_json::Error,
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
            std::ffi::CString::new(s).unwrap().as_ptr() as *mut i8, /* v is originally declared as "const char *" */
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
                    std::ffi::CString::new(k).unwrap().as_ptr() as *mut i8, /* f is originally declared as "const char *" */
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

/// Result of the imported content.
pub struct ImportedContent {
    /// Path to the imported file, absolute or relative to the process's CWD.
    pub found_here: String,
    /// Content of the imported file
    pub content: String,
}
pub type ImportCallback = fn(base: &str, base: &str) -> Result<ImportedContent, String>;

#[repr(C)]
struct ImportCallbackHolder {
    vm: *mut gojsonnet_sys::JsonnetVm,
    callback: ImportCallback,
}
unsafe extern "C" fn import_callback_bridge(
    ctx: *mut std::ffi::c_void,
    base: *const std::os::raw::c_char,
    rel: *const std::os::raw::c_char,
    found_here: *mut *mut std::os::raw::c_char,
    success: *mut std::os::raw::c_int,
) -> *mut std::os::raw::c_char {
    let holder = ctx as *mut ImportCallbackHolder;
    let vm = (*holder).vm;
    let callback = (*holder).callback;
    let base = std::ffi::CStr::from_ptr(base).to_string_lossy();
    let rel = std::ffi::CStr::from_ptr(rel).to_string_lossy();
    use std::borrow::Borrow as _;
    match callback(base.borrow(), rel.borrow()) {
        Ok(imported_content) => {
            *success = 1;
            *found_here = to_jsonnet_str(vm, &imported_content.found_here);
            to_jsonnet_str(vm, &imported_content.content)
        }
        Err(e) => {
            *success = 0;
            to_jsonnet_str(vm, &e)
        }
    }
}
unsafe fn to_jsonnet_str(
    vm: *mut gojsonnet_sys::JsonnetVm,
    rust_str: &str,
) -> *mut std::os::raw::c_char {
    let dst = gojsonnet_sys::jsonnet_realloc(vm, std::ptr::null_mut(), rust_str.len() as u64 + 1);
    std::ptr::copy_nonoverlapping(rust_str.as_ptr(), dst as *mut u8, rust_str.len());
    *dst.offset(rust_str.len() as isize) = 0;
    dst
}

impl Vm {
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            inner: unsafe { gojsonnet_sys::jsonnet_make() },
            native_callback_holders: std::collections::HashMap::new(),
            import_callback_holder: None,
        }
    }

    /// Return the version of underlying google/go-jsonnet library.
    pub fn library_version() -> String {
        let version_cstr = unsafe { std::ffi::CStr::from_ptr(gojsonnet_sys::jsonnet_version()) };
        version_cstr.to_string_lossy().into_owned()
    }

    /// Set the maximum stack depth.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.max_stack(10);
    /// ```
    pub fn max_stack(&mut self, v: u32) {
        unsafe { gojsonnet_sys::jsonnet_max_stack(self.inner, v) };
    }

    /// Evaluate a Jsonnet code and return a JSON string.
    ///
    /// ```rust
    /// let vm = gojsonnet::Vm::default();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     foo: i32,
    ///     bar: bool,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet(
    ///         "evaluate_snippet.jsonnet",
    ///         "{foo: 1+2, bar: std.isBoolean(false)}",
    ///     )
    ///     .unwrap();
    /// assert_eq!(s, S { foo: 3, bar: true });
    /// ```
    pub fn evaluate_snippet<T>(&self, filename: &str, code: &str) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        let filename_cstr = std::ffi::CString::new(filename)?;
        let code_cstr = std::ffi::CString::new(code)?;
        let mut err = 0;
        unsafe {
            let ptr = gojsonnet_sys::jsonnet_evaluate_snippet(
                self.inner,
                filename_cstr.as_ptr() as *mut i8, /* filename is originally declared as "const char *" */
                code_cstr.as_ptr() as *mut i8, // filename is originally declared as "const char *"
                &mut err,
            );
            let json_str = std::ffi::CStr::from_ptr(ptr).to_string_lossy();
            if err == 0 {
                let result = serde_json::from_str(&json_str);
                gojsonnet_sys::jsonnet_realloc(self.inner, ptr, 0);
                Ok(result?)
            } else {
                let message = json_str.into_owned();
                gojsonnet_sys::jsonnet_realloc(self.inner, ptr, 0);
                Err(Error::GoJsonnetError { message })
            }
        }
    }

    /// Register a native function to the interpreter.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.native_callback("hello", &["arg1"], |argv| {
    ///     let arg1 = argv[0].as_str().unwrap();
    ///     Some(serde_json::json!(format!("hello {}", arg1)))
    /// })
    /// .unwrap();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     message: String,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet(
    ///         "native_callback.jsonnet",
    ///         r#"local hello = std.native("hello"); {message: hello("world")}"#,
    ///     )
    ///     .unwrap();
    /// assert_eq!(
    ///     s,
    ///     S {
    ///         message: "hello world".to_owned()
    ///     }
    /// );
    /// ```
    pub fn native_callback(
        &mut self,
        name: &str,
        params: &[&str],
        callback: NativeCallback,
    ) -> Result<(), Error> {
        let name_cstr = std::ffi::CString::new(name)?;
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
        let old_holder = self.native_callback_holders.insert(name.to_owned(), holder);
        unsafe {
            if let Some(old_holder) = old_holder {
                Box::from_raw(old_holder);
            }
            gojsonnet_sys::jsonnet_native_callback(
                self.inner,
                name_cstr.as_ptr() as *mut i8, // name is originally declared as "const char *"
                Some(native_callback_bridge),
                holder as *mut std::ffi::c_void,
                params_c.as_mut_ptr(),
            );
            assert_eq!(params_c.pop(), Some(std::ptr::null_mut()));
            for param_c in params_c {
                if !param_c.is_null() {
                    std::ffi::CString::from_raw(param_c);
                }
            }
        };
        Ok(())
    }

    /// Bind a Jsonnet external variable to the given string.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.ext_var("v", "true").unwrap();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     v: String,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet("ext_var.jsonnet", "{ v: std.extVar('v') }")
    ///     .unwrap();
    /// assert_eq!(
    ///     s,
    ///     S {
    ///         v: "true".to_owned()
    ///     }
    /// );
    /// ```
    pub fn ext_var(&mut self, key: &str, val: &str) -> Result<(), Error> {
        let key_cstr = std::ffi::CString::new(key)?;
        let val_cstr = std::ffi::CString::new(val)?;
        unsafe {
            gojsonnet_sys::jsonnet_ext_var(
                self.inner,
                key_cstr.as_ptr() as *mut i8, // key is originally declared as "const char *"
                val_cstr.as_ptr() as *mut i8, // val is originally declared as "const char *"
            )
        };
        Ok(())
    }

    /// Bind a Jsonnet external variable to the given code.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.ext_code("v", "true").unwrap();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     v: bool,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet("ext_code.jsonnet", "{ v: std.extVar('v') }")
    ///     .unwrap();
    /// assert_eq!(s, S { v: true });
    /// ```
    pub fn ext_code(&mut self, key: &str, val: &str) -> Result<(), Error> {
        let key_cstr = std::ffi::CString::new(key)?;
        let val_cstr = std::ffi::CString::new(val)?;
        unsafe {
            gojsonnet_sys::jsonnet_ext_code(
                self.inner,
                key_cstr.as_ptr() as *mut i8, // key is originally declared as "const char *"
                val_cstr.as_ptr() as *mut i8, // val is originally declared as "const char *"
            )
        };
        Ok(())
    }

    /// Bind a Jsonnet top-level variable to the given string.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.tla_var("v", "true").unwrap();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     v: String,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet("tla_var.jsonnet", "function(v) { v: v }")
    ///     .unwrap();
    /// assert_eq!(
    ///     s,
    ///     S {
    ///         v: "true".to_owned()
    ///     }
    /// );
    /// ```
    pub fn tla_var(&mut self, key: &str, val: &str) -> Result<(), Error> {
        let key_cstr = std::ffi::CString::new(key)?;
        let val_cstr = std::ffi::CString::new(val)?;
        unsafe {
            gojsonnet_sys::jsonnet_tla_var(
                self.inner,
                key_cstr.as_ptr() as *mut i8, // key is originally declared as "const char *"
                val_cstr.as_ptr() as *mut i8, // val is originally declared as "const char *"
            )
        };
        Ok(())
    }

    /// Bind a Jsonnet top-level variable to the given code.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.tla_code("v", "true").unwrap();
    /// #[derive(Debug, PartialEq, serde::Deserialize)]
    /// struct S {
    ///     v: bool,
    /// }
    /// let s: S = vm
    ///     .evaluate_snippet("tla_code.jsonnet", "function(v) { v: v }")
    ///     .unwrap();
    /// assert_eq!(s, S { v: true });
    /// ```
    pub fn tla_code(&mut self, key: &str, val: &str) -> Result<(), Error> {
        let key_cstr = std::ffi::CString::new(key)?;
        let val_cstr = std::ffi::CString::new(val)?;
        unsafe {
            gojsonnet_sys::jsonnet_tla_code(
                self.inner,
                key_cstr.as_ptr() as *mut i8, // key is originally declared as "const char *"
                val_cstr.as_ptr() as *mut i8, // val is originally declared as "const char *"
            )
        };
        Ok(())
    }

    /// Add to the default import callback's library search path.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.jpath_add("/path/to/libsonnets");
    /// ```
    pub fn jpath_add(&mut self, path: &str) -> Result<(), Error> {
        let path_cstr = std::ffi::CString::new(path)?;
        unsafe {
            // path is originally declared as "const char *"
            gojsonnet_sys::jsonnet_jpath_add(self.inner, path_cstr.as_ptr() as *mut i8)
        };
        Ok(())
    }

    /// Override the callback used to locate imports.
    ///
    /// ```rust
    /// let mut vm = gojsonnet::Vm::default();
    /// vm.import_callback(|base, rel| {
    ///     Ok(gojsonnet::ImportedContent {
    ///         found_here: "import_callback.libsonnet".to_owned(),
    ///         content: "1 + 2".to_owned(),
    ///     })
    /// });
    /// let s: Vec<i32> = vm
    ///     .evaluate_snippet("import_callback.jsonnet", "[import 'foo.libsonnet']")
    ///     .unwrap();
    /// assert_eq!(s, vec![3]);
    /// ```
    pub fn import_callback(&mut self, callback: ImportCallback) {
        let holder = Box::into_raw(Box::new(ImportCallbackHolder {
            vm: self.inner,
            callback,
        }));
        let old_holder = self.import_callback_holder.replace(holder);
        unsafe {
            if let Some(old_holder) = old_holder {
                Box::from_raw(old_holder);
            }
            gojsonnet_sys::jsonnet_import_callback(
                self.inner,
                Some(import_callback_bridge),
                holder as *mut std::ffi::c_void,
            )
        };
    }
}
impl Drop for Vm {
    fn drop(&mut self) {
        unsafe {
            for (_, holder) in self.native_callback_holders.drain() {
                Box::from_raw(holder);
            }
            if let Some(holder) = self.import_callback_holder {
                Box::from_raw(holder);
            }
            gojsonnet_sys::jsonnet_destroy(self.inner)
        };
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
    fn evaluate_snippet_syntax_error() {
        let vm = super::Vm::default();
        let e = vm
            .evaluate_snippet::<()>("evaluate_snippet_syntax_error.jsonnet", "{foo: bar}")
            .unwrap_err();
        assert!(
            e.to_string()
                .starts_with("go-jsonnet returned error: evaluate_snippet_syntax_error.jsonnet:1:"),
            "e = {}",
            e
        );
        assert!(e.to_string().contains("Unknown variable"), "e = {}", e);
    }
}
