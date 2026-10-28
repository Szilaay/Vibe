//! Short, asynchronous Windows notification; never block a render or hook on audio.
pub(crate) fn play() {
    #[cfg(all(windows, not(test)))]
    unsafe {
        // MessageBeep queues the user's Windows information sound. It needs no
        // audio files, child process, window, or focus change.
        let _ = windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
            windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        );
    }
}
