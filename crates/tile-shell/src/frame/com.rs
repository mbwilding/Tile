//! The COM side of framing: `ApplicationFrameHost`'s frame manager and its frames, the handler
//! that hears a frame's close button, the window management service and the package debugger.
//!
//! None of these interfaces is documented. Their layouts come from the debug symbols of
//! Windows 11 (build 26100); the methods not called are placeholders that keep the rest in
//! their vtable slots.

use std::{
    ffi::c_void,
    sync::{Mutex, OnceLock, PoisonError},
};

use windows::{
    Win32::{
        Foundation::E_FAIL,
        System::{
            Com::{
                CLSCTX_ALL, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED,
                CoCreateFreeThreadedMarshaler, CoCreateInstance, CoInitializeEx, IStream,
                Marshal::{IMarshal, IMarshal_Impl},
            },
            WinRT::RoGetActivationFactory,
        },
        UI::Shell::{Common::IObjectArray, IPackageDebugSettings, PackageDebugSettings},
    },
    core::{
        BOOL, ComObject, GUID, HSTRING, IInspectable, IUnknown, IUnknown_Vtbl, Interface, OutRef,
        PCWSTR, Ref, Result, implement, interface,
    },
};
use windows_sys::Win32::Foundation::HWND;

use crate::win::{context, wide};

/// `ApplicationFrameHost`'s class, which makes the frame manager.
const HOST_CLASS: GUID = GUID::from_u128(0xB9B0_5098_3E30_483F_87F7_027C_A78D_A287);
/// The command a frame reports when its close button, `Alt+F4` or the system menu asks to close.
const CLOSE_COMMAND: GUID = GUID::from_u128(0x55BA_4179_C796_4426_81F0_4739_E926_5B57);
/// The window management service, whose statics tell an app its window's size.
const WINDOW_SERVICE: &str =
    "Windows.Internal.ApplicationModel.WindowManagement.UniversalAppModelInProc";

/// `IApplicationFrameManager`: makes, lists and destroys frames.
#[interface("A914F499-4633-4B26-A93E-707EB5BDC0B6")]
pub(super) unsafe trait IApplicationFrameManager: IUnknown {
    fn create_frame(&self, frame: OutRef<IApplicationFrame>) -> Result<()>;
    fn _4(&self);
    fn destroy_frame(&self, frame: Ref<IApplicationFrame>) -> Result<()>;
    fn get_frame_array(&self, frames: OutRef<IObjectArray>) -> Result<()>;
    fn register_for_frame_events(
        &self,
        handler: Ref<IApplicationFrameEventHandler>,
        cookie: *mut i32,
    ) -> Result<()>;
}

/// `IApplicationFrame`: one frame, the window with the title bar an app sits in.
#[interface("143715D9-A015-40EA-B695-D5CC267E36EE")]
pub(super) unsafe trait IApplicationFrame: IUnknown {
    fn get_frame_window(&self, window: *mut HWND) -> Result<()>;
    fn _set_position(&self);
    fn _5(&self);
    fn set_presented_window(&self, window: HWND) -> Result<()>;
    fn _7(&self);
    fn _8(&self);
    fn set_application_id(&self, id: PCWSTR, unknown: BOOL) -> Result<()>;
}

/// `IApplicationFrameEventHandler`: what a frame tells the shell about itself.
#[interface("EA5D0DE4-770D-4DA0-A9F8-D7F9A140FF79")]
pub(super) unsafe trait IApplicationFrameEventHandler: IUnknown {
    fn frame_changed(&self, frame: Ref<IApplicationFrame>) -> Result<()>;
    fn frame_command(
        &self,
        frame: Ref<IApplicationFrame>,
        command: *const GUID,
        value: i32,
    ) -> Result<()>;
    fn frame_ended(&self, frame: Ref<IApplicationFrame>) -> Result<()>;
}

/// `IUniversalAppModelInProcStatics`, the window management service's statics. It derives from
/// `IInspectable`, whose three methods come first.
#[interface("B880CE78-2E50-5A73-BFAA-A3F9A8B2F042")]
pub(super) unsafe trait IUniversalAppModelInProcStatics: IUnknown {
    fn _get_iids(&self);
    fn _get_runtime_class_name(&self);
    fn _get_trust_level(&self);
    fn _6(&self);
    fn _7(&self);
    fn _8(&self);
    fn _9(&self);
    fn _10(&self);
    fn _11(&self);
    fn _12(&self);
    fn set_bounds(&self, window: u64, rect: *const [f32; 4]) -> Result<()>;
    fn _14(&self);
    fn _15(&self);
    fn _set_is_cloaked(&self);
    fn _17(&self);
    fn _set_layout_state(&self);
    fn _19(&self);
    fn _set_placement(&self);
    fn _21(&self);
    fn _22(&self);
    fn _23(&self);
    fn _24(&self);
    fn _25(&self);
    fn _26(&self);
    fn set_visible_bounds(&self, window: u64, rect: *const [f32; 4]) -> Result<()>;
}

/// Joins the calling thread to the multithreaded apartment for the rest of its life.
pub(super) fn join_apartment() {
    // SAFETY: takes no pointers. A thread already in an apartment just stays in it.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// The frame manager, asked of `ApplicationFrameHost` directly.
pub(super) fn frame_manager() -> std::result::Result<IApplicationFrameManager, String> {
    // SAFETY: plain COM activation.
    let made = unsafe { CoCreateInstance(&HOST_CLASS, None, CLSCTX_LOCAL_SERVER) };
    context(made, "starting ApplicationFrameHost")
}

/// A new, empty frame.
pub(super) fn create_frame(
    manager: &IApplicationFrameManager,
) -> std::result::Result<IApplicationFrame, String> {
    let mut frame = None;
    // SAFETY: `frame` is a live out slot for the interface.
    let made = unsafe { manager.create_frame(&mut frame) };
    context(made, "creating the frame")?;
    frame.ok_or_else(|| "creating the frame gave nothing".into())
}

/// Destroys `frame`, with its window.
pub(super) fn destroy_frame(manager: &IApplicationFrameManager, frame: &IApplicationFrame) {
    // SAFETY: both objects are alive and belong together.
    let _ = unsafe { manager.destroy_frame(frame) };
}

/// Every frame the frame host has, those of other processes included.
pub(super) fn frames(manager: &IApplicationFrameManager) -> Vec<IApplicationFrame> {
    let mut array = None;
    // SAFETY: `array` is a live out slot for the interface.
    if unsafe { manager.get_frame_array(&mut array) }.is_err() {
        return Vec::new();
    }
    let Some(array) = array else {
        return Vec::new();
    };
    // SAFETY: plain calls on the array just returned.
    unsafe {
        let count = array.GetCount().unwrap_or(0);
        (0..count).filter_map(|i| array.GetAt(i).ok()).collect()
    }
}

/// `frame`'s window.
pub(super) fn frame_window(frame: &IApplicationFrame) -> Option<HWND> {
    let mut window: HWND = std::ptr::null_mut();
    // SAFETY: `window` is a live out pointer.
    let got = unsafe { frame.get_frame_window(&raw mut window) };
    (got.is_ok() && !window.is_null()).then_some(window)
}

/// Names `frame` after the app `id` and puts the app window `app` in it. Returns the frame's
/// window.
pub(super) fn present(
    frame: &IApplicationFrame,
    app: HWND,
    id: &str,
) -> std::result::Result<HWND, String> {
    let id = wide(id);
    // SAFETY: `id` is NUL-terminated and outlives the call; `app` is only passed on.
    unsafe {
        context(
            frame.set_application_id(PCWSTR(id.as_ptr()), true.into()),
            "naming the frame",
        )?;
        context(
            frame.set_presented_window(app),
            "putting the app in the frame",
        )?;
    }
    frame_window(frame).ok_or_else(|| "the frame has no window".into())
}

/// Names `frame` after the app `id`, without putting a window in it.
pub(super) fn name(frame: &IApplicationFrame, id: &str) -> std::result::Result<(), String> {
    let id = wide(id);
    // SAFETY: `id` is NUL-terminated and outlives the call.
    unsafe {
        context(
            frame.set_application_id(PCWSTR(id.as_ptr()), true.into()),
            "naming the frame",
        )
    }
}

/// Asks the frame host to report each frame's commands, such as close, to this process. The
/// handler is to be kept for as long as the manager.
pub(super) fn listen(manager: &IApplicationFrameManager) -> Option<IApplicationFrameEventHandler> {
    let handler = match Events::make() {
        Ok(handler) => handler,
        Err(e) => {
            tracing::warn!("couldn't make the frame event handler: {e}");
            return None;
        }
    };
    let mut cookie = 0;
    // SAFETY: the handler is alive and `cookie` is a live out pointer.
    match unsafe { manager.register_for_frame_events(&handler, &raw mut cookie) } {
        Ok(()) => tracing::info!("listening for frame commands, cookie {cookie}"),
        Err(e) => tracing::warn!("listening for frame commands: {e}"),
    }
    Some(handler)
}

/// A frame whose close was asked for.
struct CloseRequest(IApplicationFrame);

// SAFETY: the frame is a proxy to an object in another process, which any thread of this
// multithreaded apartment may call.
unsafe impl Send for CloseRequest {}

/// Frames whose close was asked for, until the framer's thread deals with them.
static CLOSE_REQUESTS: Mutex<Vec<CloseRequest>> = Mutex::new(Vec::new());

/// The frames whose close was asked for since the last call.
pub(super) fn take_close_requests() -> Vec<IApplicationFrame> {
    let requests = std::mem::take(
        &mut *CLOSE_REQUESTS
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    requests.into_iter().map(|request| request.0).collect()
}

#[implement(IApplicationFrameEventHandler, IMarshal)]
struct Events {
    /// The free-threaded marshaler aggregated into this object, which is what lets the frame host
    /// call back from any of its threads (Explorer's own objects do the same).
    ftm: OnceLock<IMarshal>,
}

impl Events {
    /// Makes the handler, with its marshaler already aggregated into it.
    fn make() -> Result<IApplicationFrameEventHandler> {
        let object = ComObject::new(Events {
            ftm: OnceLock::new(),
        });
        let outer: IUnknown = object.cast()?;
        // SAFETY: plain COM call; the marshaler holds `outer` without counting it.
        let marshaler = unsafe { CoCreateFreeThreadedMarshaler(&outer)? }.cast::<IMarshal>()?;
        let _ = object.ftm.set(marshaler);
        object.cast()
    }

    fn ftm(&self) -> Result<&IMarshal> {
        self.ftm
            .get()
            .ok_or_else(|| windows::core::Error::from(E_FAIL))
    }
}

// Lets the frame host call back from any of its threads, by marshaling like the free-threaded
// marshaler does.
impl IMarshal_Impl for Events_Impl {
    fn GetUnmarshalClass(
        &self,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> Result<GUID> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .GetUnmarshalClass(riid, Some(pv), context, Some(context_data), flags)
        }
    }

    fn GetMarshalSizeMax(
        &self,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> Result<u32> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .GetMarshalSizeMax(riid, Some(pv), context, Some(context_data), flags)
        }
    }

    fn MarshalInterface(
        &self,
        stream: Ref<IStream>,
        riid: *const GUID,
        pv: *const c_void,
        context: u32,
        context_data: *const c_void,
        flags: u32,
    ) -> Result<()> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?.MarshalInterface(
                stream.as_ref(),
                riid,
                Some(pv),
                context,
                Some(context_data),
                flags,
            )
        }
    }

    fn UnmarshalInterface(
        &self,
        stream: Ref<IStream>,
        riid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        // SAFETY: forwards COM's own arguments unchanged.
        unsafe {
            self.ftm()?
                .UnmarshalInterface(stream.as_ref(), riid, object)
        }
    }

    fn ReleaseMarshalData(&self, stream: Ref<IStream>) -> Result<()> {
        // SAFETY: forwards COM's own argument unchanged.
        unsafe { self.ftm()?.ReleaseMarshalData(stream.as_ref()) }
    }

    fn DisconnectObject(&self, reserved: u32) -> Result<()> {
        // SAFETY: forwards COM's own argument unchanged.
        unsafe { self.ftm()?.DisconnectObject(reserved) }
    }
}

impl IApplicationFrameEventHandler_Impl for Events_Impl {
    unsafe fn frame_changed(&self, _frame: Ref<IApplicationFrame>) -> Result<()> {
        Ok(())
    }

    unsafe fn frame_command(
        &self,
        frame: Ref<IApplicationFrame>,
        command: *const GUID,
        _value: i32,
    ) -> Result<()> {
        // SAFETY: the frame host passes a live GUID, or none.
        let close = unsafe { command.as_ref() } == Some(&CLOSE_COMMAND);
        if close && let Some(frame) = frame.cloned() {
            CLOSE_REQUESTS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(CloseRequest(frame));
        }
        Ok(())
    }

    unsafe fn frame_ended(&self, _frame: Ref<IApplicationFrame>) -> Result<()> {
        Ok(())
    }
}

/// The window management service's statics, which tell an app its window's size.
///
/// Without Explorer nothing else does, and a Store app keeps laying itself out for the size its
/// window was created with, which is that of the whole monitor. Only a process that hosts the
/// shell side (this one) is answered.
pub(super) fn window_statics() -> Option<IUniversalAppModelInProcStatics> {
    // SAFETY: plain WinRT activation.
    let factory: IInspectable =
        unsafe { RoGetActivationFactory(&HSTRING::from(WINDOW_SERVICE)) }.ok()?;
    factory.cast().ok()
}

/// Tells `app` that its window is `width` by `height` (and sizes the window to match).
pub(super) fn set_bounds(
    statics: &IUniversalAppModelInProcStatics,
    app: HWND,
    width: i32,
    height: i32,
) {
    let rect = [0.0, 0.0, width as f32, height as f32];
    // SAFETY: `rect` is a live local; the window is only passed on, as its id.
    unsafe {
        let _ = statics.set_bounds(app as u64, &rect);
        let _ = statics.set_visible_bounds(app as u64, &rect);
    }
}

/// Thaws the package, which Windows freezes when an app launches with no visible window.
pub(super) fn resume(package: &str) {
    let name = wide(package);
    // SAFETY: plain COM calls; `name` is NUL-terminated and outlives them.
    unsafe {
        if let Ok(settings) =
            CoCreateInstance::<_, IPackageDebugSettings>(&PackageDebugSettings, None, CLSCTX_ALL)
        {
            let _ = settings.Resume(PCWSTR(name.as_ptr()));
        }
    }
}
