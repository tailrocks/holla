use std::path::{Path, PathBuf};

use super::ScanOptions;

pub(crate) fn default_workers() -> usize {
    macos_performance_core_count()
        .or_else(|| std::thread::available_parallelism().ok().map(usize::from))
        .unwrap_or(1)
        .max(1)
}

#[cfg(target_os = "macos")]
fn macos_performance_core_count() -> Option<usize> {
    use std::{ffi::CString, mem::size_of};

    let name = CString::new("hw.perflevel0.logicalcpu").expect("static sysctl name");
    let mut value: libc::c_int = 0;
    let mut length = size_of::<libc::c_int>();
    // SAFETY: `name` is NUL-terminated, and `value`/`length` are valid writable
    // buffers of the declared size for a read-only sysctl query.
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut libc::c_int).cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && value > 0).then_some(value as usize)
}

#[cfg(not(target_os = "macos"))]
fn macos_performance_core_count() -> Option<usize> {
    None
}

#[cfg(target_os = "macos")]
pub(crate) fn init_scan_thread() -> Result<(), i32> {
    const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: libc::c_int = 3;
    const IOPOL_SCOPE_THREAD: libc::c_int = 1;
    const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: libc::c_int = 1;

    unsafe extern "C" {
        fn setiopolicy_np(
            io_type: libc::c_int,
            scope: libc::c_int,
            policy: libc::c_int,
        ) -> libc::c_int;
    }

    // Values and declaration come from the installed macOS SDK's
    // <sys/resource.h>; rust-lang/libc 0.2.186 does not expose them.
    // SAFETY: this process-global symbol accepts the three documented integer
    // constants and only changes I/O policy for the calling thread.
    let policy_result = unsafe {
        setiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_THREAD,
            IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
        )
    };
    if policy_result != 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(-1));
    }
    // SAFETY: the QoS class and relative priority are valid documented values;
    // this affects only the calling worker thread.
    let _ = unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0)
    };
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn init_scan_thread() -> Result<(), i32> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;

    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn is_dataless(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(target_os = "macos")]
pub(crate) fn default_skip_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if root == Path::new("/") {
        paths.push(PathBuf::from("/System/Volumes/Data"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Library/Mobile Documents"));
    }
    paths
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn default_skip_paths(_root: &Path) -> Vec<PathBuf> {
    Vec::new()
}

pub fn should_skip(path: &Path, options: &ScanOptions) -> bool {
    options.skip_paths.iter().any(|skip| path.starts_with(skip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_is_never_zero() {
        assert!(default_workers() >= 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn root_scan_skips_data_volume_but_subtree_scan_does_not() {
        let root = ScanOptions::new("/");
        assert!(should_skip(
            Path::new("/System/Volumes/Data/Users/person"),
            &root
        ));

        let subtree = ScanOptions::new("/tmp");
        assert!(!should_skip(
            Path::new("/System/Volumes/Data/Users/person"),
            &subtree
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mobile_documents_prefix_is_skipped() {
        let options = ScanOptions {
            root: PathBuf::from("/Users/person"),
            follow_hidden: true,
            skip_paths: vec![PathBuf::from("/Users/person/Library/Mobile Documents")],
            workers: 1,
        };

        assert!(should_skip(
            Path::new("/Users/person/Library/Mobile Documents/com~apple~CloudDocs"),
            &options
        ));
        assert!(!should_skip(
            Path::new("/Users/person/Library/Application Support"),
            &options
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_defaults_do_not_guess_mount_paths() {
        assert!(default_skip_paths(Path::new("/")).is_empty());
        assert!(default_skip_paths(Path::new("/tmp")).is_empty());
    }
}
