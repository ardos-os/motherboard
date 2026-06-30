use alloc::{ffi::CString, string::String};

pub fn cmdline_for_task(task: &kernel::task::Task) -> Option<String> {
    let raw = unsafe { bindings::kstrdup_quotable_cmdline(task.as_ptr(), bindings::GFP_KERNEL) };

    if raw.is_null() {
        panic!("BUY MORE RAM LOL")
    }

    let result = {
        let cstr = unsafe { core::ffi::CStr::from_ptr(raw as _) };
        let bytes = cstr.to_bytes();

        let owned = CString::new(bytes).ok()?;
        let string = owned.into_string().ok()?;

        string
    };

    unsafe {
        bindings::kfree(raw.cast());
    }

    Some(result)
}
