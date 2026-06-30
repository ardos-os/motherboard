use core::alloc::{GlobalAlloc, Layout};
use core::ptr::NonNull;

use kernel::alloc::Allocator;
use kernel::alloc::Flags;
use kernel::alloc::NumaNode;
use kernel::alloc::allocator::Kmalloc;

#[derive(Copy, Clone, Default)]
pub struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Usamos as Flags padrão do Kernel (GFP_KERNEL) e NO_NODE.
        // Lembra-te: GFP_KERNEL pode dormir, por isso não alocar em IRQ context.
        let flags = Flags::from(kernel::alloc::flags::GFP_KERNEL);
        let nid = NumaNode::NO_NODE;

        match Kmalloc::alloc(layout, flags, nid) {
            Ok(non_null_slice) => non_null_slice.as_ptr() as *mut u8,
            Err(_) => core::ptr::null_mut(), // Falha de OOM (Out Of Memory)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if let Some(non_null_ptr) = NonNull::new(ptr) {
            unsafe {
                Kmalloc::free(non_null_ptr, layout);
            }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let flags = Flags::from(kernel::alloc::flags::GFP_KERNEL);
        let nid = NumaNode::NO_NODE;

        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };

        let opt_ptr = NonNull::new(ptr);

        unsafe {
            match Kmalloc::realloc(opt_ptr, new_layout, layout, flags, nid) {
                Ok(new_slice) => new_slice.as_ptr() as *mut u8,
                Err(_) => core::ptr::null_mut(),
            }
        }
    }
}

// O Alocador Global oficial do Ardos OS
#[global_allocator]
pub static GLOBAL_ALLOCATOR: KernelAllocator = KernelAllocator;
#[alloc_error_handler]
fn alloc_error_handler(layout: core::alloc::Layout) -> ! {
    panic!("Failed to allocate memory in the kernel: {layout:#?}");
}

#[rustc_std_internal_symbol]
fn __rust_no_alloc_shim_is_unstable_v2() {}
