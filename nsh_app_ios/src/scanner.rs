//! Camera QR scanner for iOS: a full-screen AVFoundation capture sheet.
//!
//! Registered into the shared [`nsh_app_fiber_wallet_nsh::scan`] hook, so
//! 交易页's 扫一扫 buttons can open the camera without the shared crate
//! depending on this shell. The scanner presents a modal view controller
//! over the app's key window, runs an `AVCaptureSession` with a metadata
//! output limited to QR codes, and returns the first decoded string through
//! the request's callback (dropped on cancel, denial, or missing camera).
//!
//! Everything runs on the main thread: the shared hook is called from the
//! engine thread and hops via the dispatch main queue, and `startRunning`'s
//! brief block is masked by the sheet's fade-in animation.

use std::cell::RefCell;
use std::sync::Mutex;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureMetadataOutput,
    AVCaptureMetadataOutputObjectsDelegate, AVCaptureSession, AVCaptureVideoPreviewLayer,
    AVMediaTypeVideo, AVMetadataMachineReadableCodeObject, AVMetadataObject,
    AVMetadataObjectTypeQRCode,
};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSString};
use objc2_ui_kit::{
    UIButton, UIButtonType, UIControlEvents, UIControlState, UIColor, UIFont, UILabel,
    UIModalPresentationStyle, UIApplication, UIViewController,
};
use tracing::{info, warn};

use nsh_app_fiber_wallet_nsh::scan::{self, ScanCallback, ScanTarget};

/// The pending result of the current scan, if any.
static ACTIVE: Mutex<Option<ScanCallback>> = Mutex::new(None);

/// Registers the camera scanner into the shared hook. Call once at startup.
pub fn install() {
    scan::register(|target, callback| {
        dispatch_main(move || present_scanner(target, callback));
    });
    info!("camera QR scanner registered");
}

/// Runs `work` on the dispatch main queue.
fn dispatch_main(work: impl FnOnce() + Send + Sync + 'static) {
    dispatch2::DispatchQueue::main().exec_async(work);
}

fn hint(target: ScanTarget) -> &'static str {
    match target {
        ScanTarget::Invoice => "对准 Fiber 发票二维码",
        ScanTarget::OnchainAddress => "对准 CKB 收款地址二维码",
    }
}

/// Presents the scanner sheet (main thread only).
fn present_scanner(target: ScanTarget, callback: ScanCallback) {
    let Some(mtm) = MainThreadMarker::new() else {
        warn!("scanner present without a main-thread marker");
        return;
    };
    let Some(root) = key_window_root() else {
        warn!("scanner present without a key window root");
        return;
    };
    *ACTIVE.lock().expect("scan state poisoned") = Some(callback);
    let controller = ScannerController::new(mtm, target);
    controller.setModalPresentationStyle(UIModalPresentationStyle::FullScreen);
    root.presentViewController_animated_completion(&controller, true, None);
}

/// Drops the pending callback (cancel / dismiss paths).
fn clear_active() {
    ACTIVE.lock().expect("scan state poisoned").take();
}

/// The key window's root view controller of the app's active scene.
///
/// Dynamic getters: the scene/window classes' generated bindings carry many
/// feature flags for one lookup that has a stable message path.
fn key_window_root() -> Option<Retained<UIViewController>> {
    // SAFETY: UIKit getters on the main thread; every intermediate is
    // null-checked and the result is retained before use.
    unsafe {
        let app: Retained<AnyObject> = msg_send![UIApplication::class(), sharedApplication];
        let scenes: Retained<AnyObject> = msg_send![&app, connectedScenes];
        let enumerator: Retained<AnyObject> = msg_send![&scenes, objectEnumerator];
        loop {
            let scene: *mut AnyObject = msg_send![&enumerator, nextObject];
            if scene.is_null() {
                return None;
            }
            let is_window_scene: bool =
                msg_send![scene, isKindOfClass: objc2_ui_kit::UIWindowScene::class()];
            if !is_window_scene {
                continue;
            }
            let window: *mut AnyObject = msg_send![scene, keyWindow];
            if window.is_null() {
                continue;
            }
            let root: *mut AnyObject = msg_send![window, rootViewController];
            if root.is_null() {
                continue;
            }
            let retained: Option<Retained<AnyObject>> = Retained::retain(root);
            return retained.map(|any| Retained::cast_unchecked::<UIViewController>(any));
        }
    }
}

/// The scanner's Rust state (see `define_class!` ivars).
struct ScannerIvars {
    /// Built in `viewDidLoad`; `None` when no camera is available.
    session: RefCell<Option<Retained<AVCaptureSession>>>,
    hint_text: Retained<NSString>,
}

define_class!(
    #[unsafe(super(UIViewController))]
    #[thread_kind = MainThreadOnly]
    #[name = "FiberWalletScannerController"]
    #[ivars = ScannerIvars]
    struct ScannerController;

    unsafe impl NSObjectProtocol for ScannerController {}

    impl ScannerController {
        #[unsafe(method(viewDidLoad))]
        unsafe fn view_did_load(&self) {
            let mtm = MainThreadMarker::new().expect("viewDidLoad on the main thread");
            let view = self.view().expect("scanner view");
            let bounds = view.bounds();
            view.setBackgroundColor(Some(&UIColor::blackColor()));

            let hint_label = UILabel::initWithFrame(
                UILabel::alloc(mtm),
                CGRect::new(
                    CGPoint::new(16.0, 92.0),
                    CGSize::new(bounds.size.width - 32.0, 30.0),
                ),
            );
            hint_label.setText(Some(&self.ivars().hint_text));
            let white = UIColor::whiteColor();
            hint_label.setTextColor(Some(&white));
            hint_label.setFont(Some(&UIFont::systemFontOfSize(15.0)));
            view.addSubview(&hint_label);

            let cancel_text = NSString::from_str("取消");
            let cancel = UIButton::buttonWithType(UIButtonType::System, mtm);
            cancel.setTitle_forState(Some(&cancel_text), UIControlState::Normal);
            cancel.setTitleColor_forState(Some(&white), UIControlState::Normal);
            cancel.setFrame(CGRect::new(
                CGPoint::new(16.0, 48.0),
                CGSize::new(72.0, 36.0),
            ));
            let _: () = msg_send![
                &cancel,
                addTarget: self,
                action: sel!(scanCancelTapped:),
                forControlEvents: UIControlEvents::TouchUpInside
            ];
            view.addSubview(&cancel);

            let Some(session) = build_capture_session(&hint_label) else {
                return;
            };

            let preview = AVCaptureVideoPreviewLayer::layerWithSession(&session);
            preview.setFrame(bounds);
            view.layer().insertSublayer_atIndex(&preview, 0);

            let output = AVCaptureMetadataOutput::new();
            session.addOutput(&output);
            unsafe {
                output.setMetadataObjectsDelegate_queue(
                    Some(ProtocolObject::from_ref(self)),
                    Some(&dispatch2::DispatchQueue::main()),
                );
            }
            let qr_only = NSArray::from_slice(&[AVMetadataObjectTypeQRCode]);
            output.setMetadataObjectTypes(Some(&qr_only));

            self.ivars().session.replace(Some(session));
        }

        #[unsafe(method(viewDidAppear:))]
        unsafe fn view_did_appear(&self, _animated: objc2::runtime::Bool) {
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.startRunning();
            }
        }

        #[unsafe(method(viewDidDisappear:))]
        unsafe fn view_did_disappear(&self, _animated: objc2::runtime::Bool) {
            if let Some(session) = self.ivars().session.borrow().as_ref() {
                session.stopRunning();
            }
            clear_active();
        }

        #[unsafe(method(scanCancelTapped:))]
        fn scan_cancel_tapped(&self, _sender: *mut AnyObject) {
            clear_active();
            self.dismissViewControllerAnimated_completion(true, None);
        }
    }

    unsafe impl AVCaptureMetadataOutputObjectsDelegate for ScannerController {
        #[unsafe(method(captureOutput:didOutputMetadataObjects:fromConnection:))]
        unsafe fn capture_did_output(
            &self,
            _output: &AVCaptureMetadataOutput,
            metadata: Option<&NSArray<AVMetadataObject>>,
            _connection: &AVCaptureConnection,
        ) {
            let Some(metadata) = metadata else { return };
            let decoded = metadata
                .to_vec()
                .iter()
                .filter_map(|object| string_value(object))
                .next();
            let Some(text) = decoded else { return };
            // `take` also makes late frames no-ops while dismissing.
            let Some(callback) = ACTIVE.lock().expect("scan state poisoned").take() else {
                return;
            };
            self.dismissViewControllerAnimated_completion(true, None);
            (callback)(text);
        }
    }
);

/// Builds the session with a back-camera input. Returns `None` (with the
/// hint label explaining why) when the camera is unavailable or permission
/// was denied; creating the input is what triggers the system prompt.
fn build_capture_session(hint_label: &UILabel) -> Option<Retained<AVCaptureSession>> {
    unsafe { build_capture_session_inner(hint_label) }
}

unsafe fn build_capture_session_inner(hint_label: &UILabel) -> Option<Retained<AVCaptureSession>> {
    unsafe {
    let Some(device) =
        (unsafe { AVCaptureDevice::defaultDeviceWithMediaType(AVMediaTypeVideo.unwrap()) })
    else {
        hint_label.setText(Some(&NSString::from_str("此设备没有相机")));
        return None;
    };
    let input = (unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }).ok();
    let Some(input) = input else {
        hint_label.setText(Some(&NSString::from_str("无法访问相机：权限未授予")));
        return None;
    };
    let session = AVCaptureSession::new();
    if !session.canAddInput(&input) {
        hint_label.setText(Some(&NSString::from_str("相机输入不可用")));
        return None;
    }
    session.addInput(&input);
    Some(session)
    }
}

/// The `stringValue` of a decoded machine-readable object.
///
/// The delegate array carries the `AVMetadataObject` base class; a dynamic
/// class check avoids downcasting machinery for one property read.
fn string_value(object: &AVMetadataObject) -> Option<String> {
    // SAFETY: isKindOfClass and stringValue are plain getters.
    unsafe {
        let is_code: bool =
            msg_send![object, isKindOfClass: AVMetadataMachineReadableCodeObject::class()];
        if !is_code {
            return None;
        }
        let text: *mut NSString = msg_send![object, stringValue];
        if text.is_null() {
            return None;
        }
        Some((*text).to_string())
    }
}

impl ScannerController {
    fn new(mtm: MainThreadMarker, target: ScanTarget) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ScannerIvars {
            session: RefCell::new(None),
            hint_text: NSString::from_str(hint(target)),
        });
        unsafe { msg_send![super(this), initWithNibName: Option::<&NSString>::None, bundle: Option::<&objc2_foundation::NSBundle>::None] }
    }
}
