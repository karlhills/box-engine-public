// A minimal Windows console program (no std, no CRT), linked with rust-lld
// from macOS: the sample PE that S7 patches resources into. See build.sh.
#![no_std]
#![no_main]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup() -> u32 {
    0
}
