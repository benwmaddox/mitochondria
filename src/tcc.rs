//! TinyCC (libtcc) loaded at run time: compile C source in memory and get function pointers back.
//! The DLL and its runtime files live in `third_party/tinycc` (LGPL-2.1, see the README there); they are loaded, not linked.
use std::ffi::{c_char, c_int, c_void, CString};
use std::sync::OnceLock;

extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(m: *mut c_void, name: *const c_char) -> *mut c_void;
}

type New = unsafe extern "C" fn() -> *mut c_void;
type Delete = unsafe extern "C" fn(*mut c_void);
type SetPath = unsafe extern "C" fn(*mut c_void, *const c_char);
type SetOptions = unsafe extern "C" fn(*mut c_void, *const c_char);
type SetOutput = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
type Compile = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type AddSymbol = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_void) -> c_int;
type Relocate = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;
type GetSymbol = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
type ErrorFunc = unsafe extern "C" fn(*mut c_void, *const c_char);
type SetErrorFunc = unsafe extern "C" fn(*mut c_void, *mut c_void, ErrorFunc);

struct Lib { new: New, delete: Delete, set_lib_path: SetPath, set_options: SetOptions, set_output: SetOutput, compile: Compile, add_symbol: AddSymbol, relocate: Relocate, get_symbol: GetSymbol, set_error: SetErrorFunc, dir: String }
unsafe impl Send for Lib {}
unsafe impl Sync for Lib {}

/// Where the bundled TinyCC lives: `MITO_TCC_DIR`, or `third_party/tinycc` in this source tree.
pub fn dir() -> String { std::env::var("MITO_TCC_DIR").unwrap_or_else(|_| format!("{}/third_party/tinycc", env!("CARGO_MANIFEST_DIR"))) }

fn lib() -> Result<&'static Lib, String> {
    static L: OnceLock<Result<Lib, String>> = OnceLock::new();
    L.get_or_init(|| unsafe {
        let d = dir();
        let path: Vec<u16> = format!("{}/libtcc.dll", d).encode_utf16().chain([0]).collect();
        let m = LoadLibraryW(path.as_ptr());
        if m.is_null() { return Err(format!("could not load {}/libtcc.dll", d)); }
        let f = |n: &str| { let c = CString::new(n).unwrap(); let p = GetProcAddress(m, c.as_ptr()); if p.is_null() { Err(format!("libtcc has no {}", n)) } else { Ok(p) } };
        Ok(Lib {
            new: std::mem::transmute(f("tcc_new")?), delete: std::mem::transmute(f("tcc_delete")?), set_lib_path: std::mem::transmute(f("tcc_set_lib_path")?),
            set_options: std::mem::transmute(f("tcc_set_options")?), set_output: std::mem::transmute(f("tcc_set_output_type")?), compile: std::mem::transmute(f("tcc_compile_string")?),
            add_symbol: std::mem::transmute(f("tcc_add_symbol")?), relocate: std::mem::transmute(f("tcc_relocate")?), get_symbol: std::mem::transmute(f("tcc_get_symbol")?),
            set_error: std::mem::transmute(f("tcc_set_error_func")?), dir: d,
        })
    }).as_ref().map_err(|e| e.clone())
}

unsafe extern "C" fn on_error(opaque: *mut c_void, msg: *const c_char) {
    let out = unsafe { &mut *(opaque as *mut String) };
    out.push_str(&unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy());
    out.push('\n');
}

/// A compiled unit. Its code stays valid until it is dropped.
pub struct Unit { s: *mut c_void, errors: Box<String> }
unsafe impl Send for Unit {}

impl Unit {
    /// Check `src` with the bundled `tcc.exe` in a child process. TinyCC 0.9.27's library cannot report a compile error
    /// to a host built by MSVC/rustc (its error path crashes while unwinding), so code is checked here first and
    /// compiled in process only once it is known to compile.
    pub fn check(src: &str) -> Result<(), String> {
        use std::io::Write;
        let exe = format!("{}/tcc.exe", dir());
        let mut c = std::process::Command::new(&exe).args(["-nostdinc", "-c", "-", "-o", "nul"])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped())
            .spawn().map_err(|e| format!("could not run {}: {}", exe, e))?;
        c.stdin.take().unwrap().write_all(src.as_bytes()).map_err(|e| e.to_string())?;
        let out = c.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() { Ok(()) } else { Err(format!("C compile failed:\n{}", String::from_utf8_lossy(&out.stderr))) }
    }

    /// Compile `src` in memory (after `check`). `symbols` are host functions or data the C code may call or read by name.
    pub fn compile(src: &str, symbols: &[(&str, *const c_void)]) -> Result<Unit, String> {
        Self::check(src)?;
        let l = lib()?;
        unsafe {
            let s = (l.new)();
            if s.is_null() { return Err("tcc_new failed".into()); }
            let mut u = Unit { s, errors: Box::new(String::new()) };
            (l.set_error)(s, &mut *u.errors as *mut String as *mut c_void, on_error);
            let d = CString::new(l.dir.clone()).unwrap();
            (l.set_lib_path)(s, d.as_ptr());
            let o = CString::new("-nostdinc").unwrap(); // generated code includes nothing; it declares what it uses
            (l.set_options)(s, o.as_ptr());
            (l.set_output)(s, 1); // TCC_OUTPUT_MEMORY
            for (name, p) in symbols { let n = CString::new(*name).unwrap(); (l.add_symbol)(s, n.as_ptr(), *p); }
            let c = CString::new(src).map_err(|_| "source contains a NUL byte".to_string())?;
            if (l.compile)(s, c.as_ptr()) != 0 { return Err(format!("C compile failed:\n{}", u.errors)); }
            if (l.relocate)(s, 1 as *mut c_void) < 0 { return Err(format!("C relocation failed:\n{}", u.errors)); } // TCC_RELOCATE_AUTO
            Ok(u)
        }
    }
    /// Address of a compiled function or global.
    pub fn symbol(&self, name: &str) -> Option<*const c_void> {
        let l = lib().ok()?;
        let n = CString::new(name).ok()?;
        let p = unsafe { (l.get_symbol)(self.s, n.as_ptr()) };
        if p.is_null() { None } else { Some(p as *const c_void) }
    }
}
impl Drop for Unit { fn drop(&mut self) { if let Ok(l) = lib() { unsafe { (l.delete)(self.s) } } } }

#[cfg(test)]
mod tests {
    use super::*;

    #[repr(C)]
    struct Host { add: unsafe extern "C" fn(i64, i64) -> i64, base: *const i64 }
    unsafe extern "C" fn host_add(a: i64, b: i64) -> i64 { a + b }

    #[test]
    fn compiles_in_memory_and_calls_back_into_rust() {
        let src = "typedef long long i64;
            typedef struct { i64 (*add)(i64, i64); const i64 *base; } Host;
            i64 run(const Host *h, i64 n) { i64 t = 0; for (i64 i = 0; i < n; i++) t = h->add(t, h->base[i]); return t; }";
        let t = std::time::Instant::now();
        let u = Unit::compile(src, &[]).unwrap();
        let took = t.elapsed();
        let run: unsafe extern "C" fn(*const Host, i64) -> i64 = unsafe { std::mem::transmute(u.symbol("run").unwrap()) };
        let data = [3i64, 4, 5, 6];
        let h = Host { add: host_add, base: data.as_ptr() };
        assert_eq!(unsafe { run(&h, 4) }, 18);
        assert!(took.as_millis() < 500, "compile took {:?}", took);
    }

    #[test]
    fn reports_c_errors() {
        let e = Unit::compile("int f( { return 1; }", &[]).err().unwrap();
        assert!(e.contains("error"), "{}", e);
    }
}
