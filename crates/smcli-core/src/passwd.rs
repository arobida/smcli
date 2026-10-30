use std::ffi::{CStr, OsString};
use std::os::unix::ffi::OsStringExt;

pub(crate) fn current_user() -> Option<(OsString, OsString)> {
    let mut size = 1024;
    loop {
        let mut buffer = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        // SAFETY: all pointers are valid and the returned strings are copied before buffer is dropped.
        let status = unsafe { libc::getpwuid_r(libc::getuid(), entry.as_mut_ptr(), buffer.as_mut_ptr().cast(), buffer.len(), &mut result) };
        if status == libc::ERANGE {
            size = size.checked_mul(2)?;
            continue;
        }
        if status != 0 || result.is_null() { return None; }
        // SAFETY: successful getpwuid_r initialized entry and its non-null strings point into buffer.
        unsafe {
            let entry = entry.assume_init();
            if entry.pw_name.is_null() || entry.pw_dir.is_null() { return None; }
            return Some((OsString::from_vec(CStr::from_ptr(entry.pw_name).to_bytes().to_vec()),
                         OsString::from_vec(CStr::from_ptr(entry.pw_dir).to_bytes().to_vec())));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn concurrent_lookups_are_consistent() {
        let expected = super::current_user();
        std::thread::scope(|scope| {
            for _ in 0..16 {
                let expected = &expected;
                scope.spawn(move || for _ in 0..100 { assert_eq!(&super::current_user(), expected); });
            }
        });
    }
}
