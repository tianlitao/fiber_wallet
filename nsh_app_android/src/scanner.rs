//! Camera QR scanner for Android: a full-screen Camera2 capture overlay.
//!
//! Registered into the shared [`nsh_app_fiber_wallet_nsh::scan`] hook, so
//! 交易页's 扫一扫 buttons open the camera without the shared crate
//! depending on this shell. The session mirrors the iOS scanner: a full-screen
//! preview with a hint label and a 取消 button, and the first decoded string
//! returned through the request's callback (dropped on cancel, denial, or a
//! missing camera).
//!
//! What the iOS side does with AVFoundation happens here across three
//! layers, because the nsh Android host offers no app-level Java extension
//! point and its generated manifest only declares INTERNET (the CAMERA
//! permission is added to packaged APKs by `scripts/patch_apk_camera.sh`):
//!
//! - `bridge/scanner_bridge.dex` — one small Java class, precompiled by
//!   `scripts/build_scanner_dex.sh` and embedded here with `include_bytes!`.
//!   It tracks the resumed Activity through lifecycle callbacks, builds the
//!   overlay (SurfaceView + hint + cancel), and hands the preview Surface
//!   back through `RegisterNatives`-bound static methods. The class loads at
//!   runtime through an `InMemoryDexClassLoader`; the Rust side needs no
//!   `JNI_OnLoad` and no host cooperation.
//! - JNI ([`jni`]): the Application (via `ActivityThread`), the permission
//!   request + poll, and an `ImageReader` polled for YUV_420_888 frames.
//! - Camera2 NDK (`libcamera2ndk`): the capture session feeding the preview
//!   Surface and the ImageReader.
//!
//! Decoding is pure Rust ([`rqrr`]) over the frames' luma plane. One worker
//! thread per scan request owns the camera; UI work hops into Java.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use jni::objects::{GlobalRef, JByteBuffer, JClass, JObject, JObjectArray, JStaticMethodID, JString, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::{JNIEnv, JavaVM};
use tracing::{info, warn};

use nsh_app_fiber_wallet_nsh::scan::{self, ScanCallback, ScanTarget};

/// The precompiled glue class (see `scripts/build_scanner_dex.sh`).
const DEX: &[u8] = include_bytes!("../bridge/scanner_bridge.dex");
const BRIDGE_CLASS: &str = "dev/fiberwallet/nsh/ScannerBridge";
const CAMERA_PERMISSION: &str = "android.permission.CAMERA";
const PERMISSION_REQUEST_CODE: i32 = 4242;

/// `ImageFormat.YUV_420_888`.
const FORMAT_YUV_420_888: i32 = 0x23;
/// `CameraDevice.TEMPLATE_PREVIEW`.
const TEMPLATE_PREVIEW: u32 = 1;
/// `ACAMERA_OK`.
const ACAMERA_OK: c_int = 0;

/// Capture sizes tried in order; the emulated camera serves 1280x720, and
/// 640x480 is the API-guaranteed fallback.
const CAPTURE_SIZES: [(i32, i32); 2] = [(1280, 720), (640, 480)];

/// The pending result of the current scan, if any.
static ACTIVE: Mutex<Option<ScanCallback>> = Mutex::new(None);

/// Events the dex glue's native callbacks send to the camera thread.
enum Event {
    Surface(GlobalRef, i32, i32),
    SurfaceLost,
    Cancel,
}

static EVENTS: Mutex<Option<Sender<Event>>> = Mutex::new(None);

/// Registers the camera scanner into the shared hook. Call once at startup
/// (from `App()`); the dex glue loads lazily on the first request.
pub fn install() {
    scan::register(|target, callback| {
        let _ = std::thread::Builder::new()
            .name("camera-scanner".to_owned())
            .spawn(move || run(target, callback));
    });
    // Load the dex glue right away so its lifecycle callbacks see the host
    // activity's `onResume`: a lazy load at scan time would start tracking
    // after that resume and never learn the activity.
    let _ = std::thread::Builder::new()
        .name("scanner-bridge".to_owned())
        .spawn(|| {
            let Some(vm) = java_vm() else {
                warn!("scanner glue setup skipped: no JavaVM");
                return;
            };
            let Ok(mut env) = vm.attach_current_thread() else {
                warn!("scanner glue setup skipped: JNI attach failed");
                return;
            };
            if setup_bridge(&mut env).is_none() {
                warn!("scanner glue setup failed at startup");
            }
        });
    info!("camera QR scanner registered");
}

fn hint(target: ScanTarget) -> &'static str {
    match target {
        ScanTarget::Invoice => "对准 Fiber 发票二维码",
        ScanTarget::OnchainAddress => "对准 CKB 收款地址二维码",
    }
}

// ---------------------------------------------------------------------------
// The dex glue: load once, bind the native callbacks, track the activity.
// ---------------------------------------------------------------------------

/// The loaded glue class and its cached statics.
struct Bridge {
    class: GlobalRef,
    current_activity: JStaticMethodID,
    present: JStaticMethodID,
    dismiss: JStaticMethodID,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

/// Loads `scanner_bridge.dex` and starts activity tracking. Idempotent: the
/// first success sticks for the process.
fn setup_bridge(env: &mut JNIEnv<'_>) -> Option<&'static Bridge> {
    if let Some(bridge) = BRIDGE.get() {
        return Some(bridge);
    }
    match load_bridge(env) {
        Ok(bridge) => {
            let _ = BRIDGE.set(bridge);
            BRIDGE.get()
        }
        Err(error) => {
            warn!(error, "scanner dex glue unavailable");
            None
        }
    }
}

fn load_bridge(env: &mut JNIEnv<'_>) -> Result<Bridge, String> {
    let bytes = env
        .byte_array_from_slice(DEX)
        .map_err(|e| format!("dex byte array: {e}"))?;
    let buffer = env
        .call_static_method(
            "java/nio/ByteBuffer",
            "wrap",
            "([B)Ljava/nio/ByteBuffer;",
            &[JValue::Object(&bytes)],
        )
        .map_err(|e| cleared(env, format!("ByteBuffer.wrap: {e}")))?
        .l()
        .map_err(|e| format!("ByteBuffer.wrap result: {e}"))?;

    // The bootstrap loader suffices: the glue only references framework
    // classes, and this library was never registered with any class loader.
    let loader = env
        .new_object(
            "dalvik/system/InMemoryDexClassLoader",
            "(Ljava/nio/ByteBuffer;Ljava/lang/ClassLoader;)V",
            &[JValue::Object(&buffer), JValue::Object(&JObject::null())],
        )
        .map_err(|e| cleared(env, format!("InMemoryDexClassLoader: {e}")))?;

    let class_name = env.new_string(BRIDGE_CLASS).map_err(|e| e.to_string())?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&class_name)],
        )
        .map_err(|e| cleared(env, format!("loadClass: {e}")))?
        .l()
        .map_err(|e| format!("loadClass result: {e}"))?;
    let class = env.new_global_ref(class).map_err(|e| e.to_string())?;
    // `GlobalRef::as_obj` hands out `&JObject<'static>`; the `Into` turns it
    // into the `&JClass` the call helpers take.
    let class_ref = || -> &JClass<'static> { class.as_obj().into() };

    // The native methods the glue calls back into.
    let natives = [
        jni::NativeMethod {
            name: "nativeOnSurface".into(),
            sig: "(Landroid/view/Surface;II)V".into(),
            fn_ptr: on_surface as *mut c_void,
        },
        jni::NativeMethod {
            name: "nativeOnSurfaceLost".into(),
            sig: "()V".into(),
            fn_ptr: on_surface_lost as *mut c_void,
        },
        jni::NativeMethod {
            name: "nativeOnCancel".into(),
            sig: "()V".into(),
            fn_ptr: on_cancel as *mut c_void,
        },
    ];
    env.register_native_methods(class_ref(), &natives)
        .map_err(|e| cleared(env, format!("RegisterNatives: {e}")))?;

    // Start tracking the resumed activity (the permission request and the
    // overlay's parent view both need it).
    let application = current_application(env).ok_or_else(|| {
        "no application context: ActivityThread.currentApplication was rejected".to_owned()
    })?;
    let register = static_id(
        env,
        class_ref(),
        "register",
        "(Landroid/app/Application;)Landroid/app/Activity;",
    )?;
    // SAFETY: the method id and arguments match the glue class's statics.
    unsafe {
        env.call_static_method_unchecked(
            &class,
            &register,
            ReturnType::Object,
            &[JValue::Object(&application).as_jni()],
        )
    }
    .map_err(|e| cleared(env, format!("ScannerBridge.register: {e}")))?;

    Ok(Bridge {
        current_activity: static_id(env, class_ref(), "currentActivity", "()Landroid/app/Activity;")?,
        present: static_id(env, class_ref(), "present", "(Ljava/lang/Object;Ljava/lang/String;)V")?,
        dismiss: static_id(env, class_ref(), "dismiss", "()V")?,
        class,
    })
}

/// The glue's native callbacks. `RegisterNatives` binds them to the loaded
/// class; each runs attached on the thread that raised it.
unsafe extern "system" fn on_surface(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    surface: JObject<'_>,
    width: i32,
    height: i32,
) {
    handle_surface(env, surface, width, height);
}

unsafe extern "system" fn on_surface_lost(_env: JNIEnv<'_>, _class: JClass<'_>) {
    send(Event::SurfaceLost);
}

unsafe extern "system" fn on_cancel(_env: JNIEnv<'_>, _class: JClass<'_>) {
    send(Event::Cancel);
}

fn static_id(
    env: &mut JNIEnv<'_>,
    class: &JClass<'_>,
    name: &str,
    sig: &str,
) -> Result<JStaticMethodID, String> {
    env.get_static_method_id(class, name, sig)
        .map_err(|e| format!("{name}{sig}: {e}"))
}

/// `ActivityThread.currentApplication()`, with `AppGlobals` as the fallback.
/// Both are hidden but light-grey APIs (reflection allowed): from a
/// dlopen'd library the application context is reachable no other way.
pub(crate) fn current_application<'a>(env: &mut JNIEnv<'a>) -> Option<JObject<'a>> {
    for (class, method) in [
        ("android/app/ActivityThread", "currentApplication"),
        ("android/app/AppGlobals", "getInitialApplication"),
    ] {
        match env
            .call_static_method(class, method, "()Landroid/app/Application;", &[])
            .and_then(|value| value.l())
        {
            Ok(app) if !app.is_null() => return Some(app),
            Ok(_) => continue,
            Err(error) => {
                let _ = env.exception_clear();
                warn!(class, method, %error, "application lookup failed");
            }
        }
    }
    None
}

/// Calls a cached static that returns an object, clearing Java exceptions.
fn call_static_object<'a>(
    env: &mut JNIEnv<'a>,
    bridge: &'static Bridge,
    method: &JStaticMethodID,
    name: &str,
) -> Option<JObject<'a>> {
    let result = unsafe {
        env.call_static_method_unchecked(&bridge.class, method, ReturnType::Object, &[])
    };
    match result.and_then(|value| value.l()) {
        Ok(object) if !object.is_null() => Some(object),
        Ok(_) => None,
        Err(error) => {
            let _ = env.exception_clear();
            warn!(%error, name, "static call failed");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// A scan request, end to end.
// ---------------------------------------------------------------------------

fn run(target: ScanTarget, callback: ScanCallback) {
    let Some(vm) = java_vm() else {
        warn!("no JavaVM (JNI_GetCreatedJavaVMs unavailable)");
        return;
    };
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(error) => {
            warn!(%error, "scanner thread attach failed");
            return;
        }
    };
    let Some(bridge) = setup_bridge(&mut env) else {
        return;
    };
    // The glue tracks activities through lifecycle callbacks; eager setup at
    // startup makes that land before the scan, but absorb races with a poll.
    let activity = (|| {
        for _ in 0..20 {
            if let Some(activity) =
                call_static_object(&mut env, bridge, &bridge.current_activity, "currentActivity")
            {
                return Some(activity);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        None
    })();
    let Some(activity) = activity else {
        warn!("no resumed activity to scan from");
        return;
    };
    let Ok(activity) = env.new_global_ref(&activity) else {
        warn!("activity global ref failed");
        return;
    };

    *ACTIVE.lock().expect("scan state poisoned") = Some(callback);
    let (tx, rx) = mpsc::channel::<Event>();
    *EVENTS.lock().expect("scan events poisoned") = Some(tx);

    // The overlay goes up first (black sheet + hint); the permission dialog
    // and the preview appear over it. The event sender is live before the
    // view is laid out, so early `surfaceChanged` callbacks are never lost.
    let Ok(hint_text) = env.new_string(hint(target)) else {
        warn!("hint string failed");
        shutdown(&mut env, bridge, Some(&rx));
        return;
    };
    if let Err(error) = unsafe {
        env.call_static_method_unchecked(
            &bridge.class,
            &bridge.present,
            ReturnType::Primitive(Primitive::Void),
            &[
                JValue::Object(activity.as_obj()).as_jni(),
                JValue::Object(&hint_text).as_jni(),
            ],
        )
    } {
        let _ = env.exception_clear();
        warn!(%error, target = ?target, "scanner overlay did not present");
        shutdown(&mut env, bridge, Some(&rx));
        return;
    }

    // The preview surface, within a few seconds of the view laying out.
    let surface = loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Event::Surface(surface, _, _)) => break Some(surface),
            Ok(_) | Err(_) => break None,
        }
    };
    let Some(surface) = surface else {
        warn!("preview surface never became available");
        shutdown(&mut env, bridge, Some(&rx));
        return;
    };

    if !ensure_camera_permission(&mut env, &activity, &rx) {
        warn!("camera permission not granted");
        shutdown(&mut env, bridge, Some(&rx));
        return;
    }

    let decoded = match unsafe { Camera::start(&mut env, &surface) } {
        Ok(mut camera) => {
            info!("camera session running");
            let decoded = camera.run(&mut env, &rx);
            unsafe { camera.stop(&mut env) };
            match decoded {
                Ok(decoded) => decoded,
                Err(error) => {
                    warn!(%error, "frame loop failed");
                    None
                }
            }
        }
        Err(error) => {
            warn!(%error, "camera session failed to start");
            None
        }
    };

    // The callback is claimed before teardown: `shutdown` clears it too.
    let callback = ACTIVE.lock().expect("scan state poisoned").take();
    shutdown(&mut env, bridge, Some(&rx));
    if let (Some(text), Some(callback)) = (decoded, callback) {
        info!("scan complete");
        callback(text);
    }
}

/// Requests the CAMERA permission and polls for the grant. The result lands
/// in the host Activity's permission callback, which this shell does not
/// control, so polling is the only way to observe it.
fn ensure_camera_permission(
    env: &mut JNIEnv<'_>,
    activity: &GlobalRef,
    rx: &Receiver<Event>,
) -> bool {
    let Ok(permission) = env.new_string(CAMERA_PERMISSION) else {
        return false;
    };
    let mut requested = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if cancelled(rx) {
            return false;
        }
        let granted = env
            .call_method(
                activity.as_obj(),
                "checkSelfPermission",
                "(Ljava/lang/String;)I",
                &[JValue::Object(&permission)],
            )
            .and_then(|value| value.i())
            .map(|code| code == 0)
            .unwrap_or(false);
        if granted {
            return true;
        }
        if !requested {
            requested = true;
            if let Ok(array) = env.new_object_array(1, "java/lang/String", &permission) {
                let _ = env.call_method(
                    activity.as_obj(),
                    "requestPermissions",
                    "([Ljava/lang/String;I)V",
                    &[JValue::Object(&array), JValue::Int(PERMISSION_REQUEST_CODE)],
                );
                info!("camera permission requested");
            }
        }
        if Instant::now() > deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Non-blocking check for a cancel / surface-lost event.
fn cancelled(rx: &Receiver<Event>) -> bool {
    matches!(
        rx.try_recv(),
        Ok(Event::Cancel | Event::SurfaceLost) | Err(TryRecvError::Disconnected)
    )
}

// ---------------------------------------------------------------------------
// The camera: Camera2 NDK session + polled ImageReader frames.
// ---------------------------------------------------------------------------

/// One Camera2 session and its polled ImageReader. Every field starts null
/// so [`Camera::stop`] can run at any build stage and free what exists.
struct Camera {
    manager: *mut ACameraManager,
    device: *mut ACameraDevice,
    session: *mut ACameraCaptureSession,
    request: *mut ACaptureRequest,
    container: *mut ACaptureSessionOutputContainer,
    preview_window: *mut ANativeWindow,
    preview_output: *mut ACaptureSessionOutput,
    preview_target: *mut ACameraOutputTarget,
    reader_window: *mut ANativeWindow,
    reader_output: *mut ACaptureSessionOutput,
    reader_target: *mut ACameraOutputTarget,
    reader: Option<GlobalRef>,
    width: i32,
    height: i32,
}

impl Camera {
    /// Opens the back camera into a two-target repeating session: the
    /// preview Surface and an ImageReader for luma frames. The first capture
    /// size the device accepts wins (session creation validates it).
    unsafe fn start(env: &mut JNIEnv<'_>, surface: &GlobalRef) -> Result<Camera, String> {
        let mut last = Err("no capture size configured".to_owned());
        for (width, height) in CAPTURE_SIZES {
            match unsafe { Camera::try_start(env, surface, width, height) } {
                Ok(camera) => return Ok(camera),
                Err(error) => last = Err(error),
            }
        }
        last
    }

    unsafe fn try_start(
        env: &mut JNIEnv<'_>,
        surface: &GlobalRef,
        width: i32,
        height: i32,
    ) -> Result<Camera, String> {
        let mut camera = Camera {
            manager: std::ptr::null_mut(),
            device: std::ptr::null_mut(),
            session: std::ptr::null_mut(),
            request: std::ptr::null_mut(),
            container: std::ptr::null_mut(),
            preview_window: std::ptr::null_mut(),
            preview_output: std::ptr::null_mut(),
            preview_target: std::ptr::null_mut(),
            reader_window: std::ptr::null_mut(),
            reader_output: std::ptr::null_mut(),
            reader_target: std::ptr::null_mut(),
            reader: None,
            width,
            height,
        };
        match unsafe { camera.build(env, surface) } {
            Ok(()) => Ok(camera),
            Err(error) => {
                unsafe { camera.stop(env) };
                Err(error)
            }
        }
    }

    unsafe fn build(&mut self, env: &mut JNIEnv<'_>, surface: &GlobalRef) -> Result<(), String> {
        unsafe {
            self.manager = ACameraManager_create();
            if self.manager.is_null() {
                return Err("ACameraManager_create failed".into());
            }

            // The reader is a Java object: its Surface joins the session as a
            // second target and its frames are polled over JNI. Its locals
            // stay inside this frame; only the GlobalRef outlives it.
            let reader = env
                .call_static_method(
                    "android/media/ImageReader",
                    "newInstance",
                    "(IIII)Landroid/media/ImageReader;",
                    &[
                        JValue::Int(self.width),
                        JValue::Int(self.height),
                        JValue::Int(FORMAT_YUV_420_888),
                        JValue::Int(4),
                    ],
                )
                .map_err(|e| cleared(env, format!("ImageReader.newInstance: {e}")))?
                .l()
                .map_err(|e| format!("newInstance result: {e}"))?;
            let reader = env.new_global_ref(reader).map_err(|e| e.to_string())?;
            let reader_surface = env
                .call_method(reader.as_obj(), "getSurface", "()Landroid/view/Surface;", &[])
                .map_err(|e| cleared(env, format!("getSurface: {e}")))?
                .l()
                .map_err(|e| format!("getSurface result: {e}"))?;
            self.reader = Some(reader);
            self.preview_window =
                ANativeWindow_from_surface(env.get_raw(), surface.as_obj().as_raw());
            self.reader_window =
                ANativeWindow_from_surface(env.get_raw(), reader_surface.as_raw());
            if self.preview_window.is_null() || self.reader_window.is_null() {
                return Err("ANativeWindow_fromSurface failed".into());
            }

            let id = first_camera_id(self.manager).ok_or_else(|| "no camera available".to_owned())?;
            self.device = open_camera(self.manager, &id)?;

            status_of(
                ACaptureSessionOutputContainer_create(&mut self.container),
                "OutputContainer_create",
            )?;
            status_of(
                ACaptureSessionOutput_create(self.preview_window, &mut self.preview_output),
                "Output_create(preview)",
            )?;
            status_of(
                ACaptureSessionOutput_create(self.reader_window, &mut self.reader_output),
                "Output_create(reader)",
            )?;
            status_of(
                ACaptureSessionOutputContainer_add(self.container, self.preview_output),
                "Container_add(preview)",
            )?;
            status_of(
                ACaptureSessionOutputContainer_add(self.container, self.reader_output),
                "Container_add(reader)",
            )?;

            status_of(
                ACameraDevice_createCaptureRequest(self.device, TEMPLATE_PREVIEW, &mut self.request),
                "createCaptureRequest",
            )?;
            status_of(
                ACameraOutputTarget_create(self.preview_window, &mut self.preview_target),
                "OutputTarget_create(preview)",
            )?;
            status_of(
                ACameraOutputTarget_create(self.reader_window, &mut self.reader_target),
                "OutputTarget_create(reader)",
            )?;
            status_of(
                ACaptureRequest_addTarget(self.request, self.preview_target),
                "addTarget(preview)",
            )?;
            status_of(
                ACaptureRequest_addTarget(self.request, self.reader_target),
                "addTarget(reader)",
            )?;

            let session_callbacks = ACameraCaptureSession_stateCallbacks {
                context: std::ptr::null_mut(),
                onClosed: Some(session_closed),
                onReady: Some(session_ready),
                onActive: Some(session_active),
            };
            status_of(
                ACameraDevice_createCaptureSession(
                    self.device,
                    self.container,
                    &session_callbacks,
                    &mut self.session,
                ),
                "createCaptureSession",
            )?;

            let capture_callbacks = ACameraCaptureSession_captureCallbacks {
                context: std::ptr::null_mut(),
                onCaptureStarted: None,
                onCaptureProgressed: None,
                onCaptureCompleted: None,
                onCaptureFailed: None,
                onCaptureSequenceCompleted: None,
                onCaptureSequenceAborted: None,
                onCaptureBufferLost: None,
            };
            status_of(
                ACameraCaptureSession_setRepeatingRequest(
                    self.session,
                    &capture_callbacks,
                    1,
                    [self.request].as_ptr(),
                    std::ptr::null_mut(),
                ),
                "setRepeatingRequest",
            )?;
            Ok(())
        }
    }

    /// The frame loop: poll the reader, decode, watch for cancel.
    fn run(&mut self, env: &mut JNIEnv<'_>, rx: &Receiver<Event>) -> Result<Option<String>, String> {
        let mut last_decode = Instant::now() - Duration::from_secs(1);
        loop {
            if cancelled(rx) {
                return Ok(None);
            }
            if last_decode.elapsed() >= Duration::from_millis(150) {
                last_decode = Instant::now();
                if let Some(text) = self.decode_latest(env) {
                    return Ok(Some(text));
                }
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }

    /// Copies the newest queued frame's luma plane and runs the decoder.
    fn decode_latest(&mut self, env: &mut JNIEnv<'_>) -> Option<String> {
        let reader = self.reader.as_ref()?;
        let image = env
            .call_method(
                reader.as_obj(),
                "acquireLatestImage",
                "()Landroid/media/Image;",
                &[],
            )
            .ok()
            .and_then(|value| value.l().ok())?;
        let decoded = decode_frame(env, &image, self.width, self.height);
        let _ = env.call_method(&image, "close", "()V", &[]);
        decoded
    }

    /// Stops the session and releases every camera-side handle. Safe to run
    /// at any stage: null handles are skipped.
    unsafe fn stop(&mut self, env: &mut JNIEnv<'_>) {
        unsafe {
            if !self.session.is_null() {
                ACameraCaptureSession_stopRepeating(self.session);
                ACameraCaptureSession_close(self.session);
                self.session = std::ptr::null_mut();
            }
            if !self.device.is_null() {
                ACameraDevice_close(self.device);
                self.device = std::ptr::null_mut();
            }
            if !self.request.is_null() {
                ACaptureRequest_free(self.request);
                self.request = std::ptr::null_mut();
            }
            if !self.preview_target.is_null() {
                ACameraOutputTarget_free(self.preview_target);
                self.preview_target = std::ptr::null_mut();
            }
            if !self.reader_target.is_null() {
                ACameraOutputTarget_free(self.reader_target);
                self.reader_target = std::ptr::null_mut();
            }
            if !self.preview_output.is_null() {
                ACaptureSessionOutput_free(self.preview_output);
                self.preview_output = std::ptr::null_mut();
            }
            if !self.reader_output.is_null() {
                ACaptureSessionOutput_free(self.reader_output);
                self.reader_output = std::ptr::null_mut();
            }
            if !self.container.is_null() {
                ACaptureSessionOutputContainer_free(self.container);
                self.container = std::ptr::null_mut();
            }
            if !self.preview_window.is_null() {
                ANativeWindow_release(self.preview_window);
                self.preview_window = std::ptr::null_mut();
            }
            if !self.reader_window.is_null() {
                ANativeWindow_release(self.reader_window);
                self.reader_window = std::ptr::null_mut();
            }
            if let Some(reader) = self.reader.take() {
                let _ = env.call_method(reader.as_obj(), "close", "()V", &[]);
            }
            if !self.manager.is_null() {
                ACameraManager_delete(self.manager);
                self.manager = std::ptr::null_mut();
            }
        }
    }
}

// The ImageReader is created inline in `Camera::build`: its Surface local
// must not outlive the JNI frame it was made in.

/// Copies the Y plane out of one Image and runs the QR decoder over it.
fn decode_frame(
    env: &mut JNIEnv<'_>,
    image: &JObject<'_>,
    width: i32,
    height: i32,
) -> Option<String> {
    let planes = env
        .call_method(image, "getPlanes", "()[Landroid/media/Image$Plane;", &[])
        .ok()
        .and_then(|value| value.l().ok())?;
    let plane0 = env
        .get_object_array_element(&JObjectArray::from(planes), 0)
        .ok()
        .map(JObject::from)?;
    let row_stride = env
        .call_method(&plane0, "getRowStride", "()I", &[])
        .ok()
        .and_then(|value| value.i().ok())? as usize;
    let buffer = env
        .call_method(&plane0, "getBuffer", "()Ljava/nio/ByteBuffer;", &[])
        .ok()
        .and_then(|value| value.l().ok())?;
    let buffer = JByteBuffer::from(buffer);
    let address = env.get_direct_buffer_address(&buffer).ok()?;

    let width = width as usize;
    let height = height as usize;
    let needed = row_stride * (height - 1) + width;
    if env.get_direct_buffer_capacity(&buffer).unwrap_or(0) < needed {
        return None;
    }
    let mut luma = Vec::with_capacity(width * height);
    for row in 0..height {
        luma.extend_from_slice(unsafe {
            std::slice::from_raw_parts(address.add(row * row_stride), width)
        });
    }
    crate::qr::decode_luma(width, height, row_stride, &luma)
}
fn first_camera_id(manager: *mut ACameraManager) -> Option<Vec<u8>> {
    unsafe {
        let mut list: *mut ACameraIdList = std::ptr::null_mut();
        if ACameraManager_getCameraIdList(manager, &mut list) != ACAMERA_OK || list.is_null() {
            return None;
        }
        let id = ((*list).numCameras > 0 && !(*list).cameraIds.is_null())
            .then(|| CStr::from_ptr(*(*list).cameraIds).to_bytes().to_vec());
        ACameraManager_deleteCameraIdList(list);
        id
    }
}

fn open_camera(manager: *mut ACameraManager, id: &[u8]) -> Result<*mut ACameraDevice, String> {
    unsafe {
        let mut device: *mut ACameraDevice = std::ptr::null_mut();
        let callbacks = ACameraDevice_stateCallbacks {
            context: std::ptr::null_mut(),
            onDisconnected: Some(device_disconnected),
            onError: Some(device_error),
        };
        let mut id_buf = id.to_vec();
        id_buf.push(0);
        status_of(
            ACameraManager_openCamera(
                manager,
                id_buf.as_ptr() as *const c_char,
                &callbacks,
                &mut device,
            ),
            "openCamera",
        )?;
        if device.is_null() {
            return Err("openCamera returned no device".into());
        }
        Ok(device)
    }
}

fn status_of(status: c_int, what: &str) -> Result<(), String> {
    (status == ACAMERA_OK)
        .then_some(())
        .ok_or_else(|| format!("{what}: camera status {status}"))
}

// The callbacks only exist to log; the frame loop owns the lifecycle.
unsafe extern "C" fn device_disconnected(_context: *mut c_void, _device: *mut ACameraDevice) {
    warn!("camera device disconnected");
}

unsafe extern "C" fn device_error(
    _context: *mut c_void,
    _device: *mut ACameraDevice,
    error: c_int,
) {
    warn!(error, "camera device error");
}

unsafe extern "C" fn session_closed(_context: *mut c_void, _session: *mut ACameraCaptureSession) {}
unsafe extern "C" fn session_ready(_context: *mut c_void, _session: *mut ACameraCaptureSession) {}
unsafe extern "C" fn session_active(_context: *mut c_void, _session: *mut ACameraCaptureSession) {}

// ---------------------------------------------------------------------------
// Teardown shared by every exit path.
// ---------------------------------------------------------------------------

fn shutdown(env: &mut JNIEnv<'_>, bridge: &'static Bridge, rx: Option<&Receiver<Event>>) {
    if let Some(rx) = rx {
        while rx.try_recv().is_ok() {}
    }
    *EVENTS.lock().expect("scan events poisoned") = None;
    unsafe {
        env.call_static_method_unchecked(
            &bridge.class,
            &bridge.dismiss,
            ReturnType::Primitive(Primitive::Void),
            &[],
        )
    }
    .ok();
    let _ = env.exception_clear();
    ACTIVE.lock().expect("scan state poisoned").take();
}

fn send(event: Event) {
    if let Some(tx) = EVENTS.lock().expect("scan events poisoned").as_ref() {
        let _ = tx.send(event);
    }
}

/// Handles `ScannerBridge.nativeOnSurface` (runs on the UI thread).
fn handle_surface(env: JNIEnv<'_>, surface: JObject<'_>, width: i32, height: i32) {
    let mut env = env;
    if let Ok(surface) = env.new_global_ref(surface) {
        send(Event::Surface(surface, width, height));
    }
}

/// Clears a pending Java exception, returning its message for the error log.
pub(crate) fn exception_text(env: &mut JNIEnv<'_>) -> Option<String> {
    let throwable = env.exception_occurred().ok()?;
    if throwable.is_null() {
        return None;
    }
    // Clear first: any further JNI call would itself fail while the
    // exception is pending; the local ref stays valid afterwards.
    let _ = env.exception_clear();
    let text = env
        .call_method(&throwable, "toString", "()Ljava/lang/String;", &[])
        .ok()
        .and_then(|value| value.l().ok())
        .map(JString::from)?;
    let message = env
        .get_string(&text)
        .ok()
        .map(|message| message.to_string_lossy().into_owned());
    message
}

fn cleared(env: &mut JNIEnv<'_>, message: String) -> String {
    if let Some(exception) = exception_text(env) {
        return format!("{message}: {exception}");
    }
    let _ = env.exception_clear();
    message
}

/// The ART VM of this process, fetched before any `System.loadLibrary` of
/// our own (the dev host dlopens this library, so there is no `JNI_OnLoad`).
///
/// `dlsym(RTLD_DEFAULT, …)` misses `libart.so` inside the app's linker
/// namespace on some Android versions, so the lookup falls back to opening
/// the runtime libraries by name.
pub(crate) fn java_vm() -> Option<JavaVM> {
    unsafe {
        let get = |symbol: *mut c_void| -> Option<JavaVM> {
            if symbol.is_null() {
                return None;
            }
            let get: unsafe extern "C" fn(*mut *mut jni::sys::JavaVM, c_int, *mut c_int) -> c_int =
                std::mem::transmute(symbol);
            let mut vm: *mut jni::sys::JavaVM = std::ptr::null_mut();
            let mut count: c_int = 0;
            get(&mut vm, 1, &mut count);
            if count < 1 || vm.is_null() {
                return None;
            }
            JavaVM::from_raw(vm).ok()
        };

        let name = c"JNI_GetCreatedJavaVMs".as_ptr();
        if let Some(vm) = get(libc::dlsym(libc::RTLD_DEFAULT, name)) {
            return Some(vm);
        }
        warn!("JNI_GetCreatedJavaVMs not in the default scope; opening libart.so");
        for library in [c"libart.so", c"libnativehelper.so"] {
            let handle = libc::dlopen(library.as_ptr(), libc::RTLD_NOW);
            if handle.is_null() {
                warn!(library = library.to_string_lossy().as_ref(), "dlopen failed");
                continue;
            }
            if let Some(vm) = get(libc::dlsym(handle, name)) {
                return Some(vm);
            }
        }
        warn!("no created JavaVM found through any lookup");
        None
    }
}

// ---------------------------------------------------------------------------
// Camera2 NDK bindings (the pieces this scanner uses).
// ---------------------------------------------------------------------------

#[allow(non_camel_case_types)]
type ACameraManager = c_void;
#[allow(non_camel_case_types)]
type ACameraDevice = c_void;
#[allow(non_camel_case_types)]
type ACameraCaptureSession = c_void;
#[allow(non_camel_case_types)]
type ACaptureRequest = c_void;
#[allow(non_camel_case_types)]
type ACaptureSessionOutputContainer = c_void;
#[allow(non_camel_case_types)]
type ACaptureSessionOutput = c_void;
#[allow(non_camel_case_types)]
type ACameraOutputTarget = c_void;
#[allow(non_camel_case_types)]
type ANativeWindow = c_void;

#[repr(C)]
struct ACameraIdList {
    numCameras: c_int,
    cameraIds: *mut *const c_char,
}

#[repr(C)]
struct ACameraDevice_stateCallbacks {
    context: *mut c_void,
    onDisconnected: Option<unsafe extern "C" fn(*mut c_void, *mut ACameraDevice)>,
    onError: Option<unsafe extern "C" fn(*mut c_void, *mut ACameraDevice, c_int)>,
}

#[repr(C)]
struct ACameraCaptureSession_stateCallbacks {
    context: *mut c_void,
    onClosed: Option<unsafe extern "C" fn(*mut c_void, *mut ACameraCaptureSession)>,
    onReady: Option<unsafe extern "C" fn(*mut c_void, *mut ACameraCaptureSession)>,
    onActive: Option<unsafe extern "C" fn(*mut c_void, *mut ACameraCaptureSession)>,
}

#[repr(C)]
#[allow(dead_code)]
struct ACameraCaptureSession_captureCallbacks {
    context: *mut c_void,
    onCaptureStarted: Option<unsafe extern "C" fn()>,
    onCaptureProgressed: Option<unsafe extern "C" fn()>,
    onCaptureCompleted: Option<unsafe extern "C" fn()>,
    onCaptureFailed: Option<unsafe extern "C" fn()>,
    onCaptureSequenceCompleted: Option<unsafe extern "C" fn()>,
    onCaptureSequenceAborted: Option<unsafe extern "C" fn()>,
    onCaptureBufferLost: Option<unsafe extern "C" fn()>,
}

#[link(name = "camera2ndk")]
unsafe extern "C" {
    fn ACameraManager_create() -> *mut ACameraManager;
    fn ACameraManager_delete(manager: *mut ACameraManager);
    fn ACameraManager_getCameraIdList(manager: *mut ACameraManager, list: *mut *mut ACameraIdList) -> c_int;
    fn ACameraManager_deleteCameraIdList(list: *mut ACameraIdList);
    fn ACameraManager_openCamera(
        manager: *mut ACameraManager,
        id: *const c_char,
        callbacks: *const ACameraDevice_stateCallbacks,
        device: *mut *mut ACameraDevice,
    ) -> c_int;
    fn ACameraDevice_close(device: *mut ACameraDevice) -> c_int;
    fn ACameraDevice_createCaptureRequest(
        device: *mut ACameraDevice,
        template: u32,
        request: *mut *mut ACaptureRequest,
    ) -> c_int;
    fn ACameraDevice_createCaptureSession(
        device: *mut ACameraDevice,
        outputs: *const ACaptureSessionOutputContainer,
        callbacks: *const ACameraCaptureSession_stateCallbacks,
        session: *mut *mut ACameraCaptureSession,
    ) -> c_int;
    fn ACameraCaptureSession_setRepeatingRequest(
        session: *mut ACameraCaptureSession,
        callbacks: *const ACameraCaptureSession_captureCallbacks,
        num_requests: c_int,
        requests: *const *mut ACaptureRequest,
        last_capture_sequence_id: *mut c_int,
    ) -> c_int;
    fn ACameraCaptureSession_stopRepeating(session: *mut ACameraCaptureSession) -> c_int;
    fn ACameraCaptureSession_close(session: *mut ACameraCaptureSession);
    fn ACaptureRequest_addTarget(
        request: *mut ACaptureRequest,
        target: *const ACameraOutputTarget,
    ) -> c_int;
    fn ACaptureRequest_free(request: *mut ACaptureRequest);
    fn ACaptureSessionOutputContainer_create(
        container: *mut *mut ACaptureSessionOutputContainer,
    ) -> c_int;
    fn ACaptureSessionOutputContainer_free(container: *mut ACaptureSessionOutputContainer);
    fn ACaptureSessionOutputContainer_add(
        container: *mut ACaptureSessionOutputContainer,
        output: *mut ACaptureSessionOutput,
    ) -> c_int;
    fn ACaptureSessionOutput_create(
        window: *mut ANativeWindow,
        output: *mut *mut ACaptureSessionOutput,
    ) -> c_int;
    fn ACaptureSessionOutput_free(output: *mut ACaptureSessionOutput);
    fn ACameraOutputTarget_create(
        window: *mut ANativeWindow,
        target: *mut *mut ACameraOutputTarget,
    ) -> c_int;
    fn ACameraOutputTarget_free(target: *mut ACameraOutputTarget);
}

#[link(name = "android")]
unsafe extern "C" {
    #[link_name = "ANativeWindow_fromSurface"]
    fn ANativeWindow_from_surface(
        env: *mut jni::sys::JNIEnv,
        surface: jni::sys::jobject,
    ) -> *mut ANativeWindow;
    #[link_name = "ANativeWindow_release"]
    fn ANativeWindow_release(window: *mut ANativeWindow);
}
