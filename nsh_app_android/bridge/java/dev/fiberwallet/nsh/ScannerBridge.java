package dev.fiberwallet.nsh;

import android.app.Activity;
import android.app.Application;
import android.graphics.Color;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.view.Gravity;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewGroup;
import android.widget.Button;
import android.widget.FrameLayout;
import android.widget.TextView;

/**
 * Java-side glue for the Rust camera QR scanner (nsh_app_android/src/scanner.rs).
 *
 * The nsh Android host ships no app-level Java extension point: an app's Rust
 * code runs as a native library inside the host Activity, and the only
 * manifest the tooling generates declares INTERNET. This class is compiled
 * once into {@code bridge/scanner_bridge.dex} (scripts/build_scanner_dex.sh),
 * embedded in the Rust binary with include_bytes!, and loaded at runtime
 * through an InMemoryDexClassLoader; the Rust side then binds the three
 * {@code native…} methods with RegisterNatives. It provides what raw JNI
 * cannot implement without a real class:
 *
 * - the ActivityLifecycleCallbacks that track the resumed Activity (Rust
 *   reaches the host only through the Application context);
 * - the scanner overlay: a full-screen SurfaceView (camera preview), the
 *   hint label and the 取消 button, added over the app's content view;
 * - the SurfaceHolder.Callback that hands the preview Surface to Rust.
 *
 * The runtime CAMERA permission is requested with Activity.requestPermissions;
 * the result callback arrives at the host Activity, which we do not control,
 * so Rust polls checkSelfPermission instead of waiting for a result push.
 */
public final class ScannerBridge
        implements Application.ActivityLifecycleCallbacks, SurfaceHolder.Callback {

    private static final ScannerBridge INSTANCE = new ScannerBridge();

    /** The most recently resumed host Activity, or null. */
    private Activity activity;
    /** The scanner overlay, while the scanner is up. */
    private FrameLayout overlay;
    private SurfaceView preview;

    private ScannerBridge() {}

    // ---- entry points called from Rust (any thread) ----

    /**
     * Registers the lifecycle tracker with the application. Returns the
     * resumed Activity right away, or null when none is up yet.
     */
    public static Activity register(Application application) {
        application.registerActivityLifecycleCallbacks(INSTANCE);
        return INSTANCE.activity;
    }

    /**
     * The most recently resumed Activity, or null. Lifecycle callbacks only
     * report future transitions — a scan request can arrive before the first
     * one — so this falls back to walking ActivityThread's activity records
     * (hidden but light-grey APIs: reflection on them is allowed).
     */
    public static Activity currentActivity() {
        if (INSTANCE.activity != null && !INSTANCE.activity.isFinishing()) {
            return INSTANCE.activity;
        }
        try {
            Class<?> threadClass = Class.forName("android.app.ActivityThread");
            Object thread = threadClass.getMethod("currentActivityThread").invoke(null);
            java.lang.reflect.Field records = threadClass.getDeclaredField("mActivities");
            records.setAccessible(true);
            android.util.ArrayMap<?, ?> activities = (android.util.ArrayMap<?, ?>) records.get(thread);
            if (activities != null) {
                for (Object record : activities.values()) {
                    java.lang.reflect.Field paused = record.getClass().getDeclaredField("paused");
                    paused.setAccessible(true);
                    if (paused.getBoolean(record)) {
                        continue;
                    }
                    java.lang.reflect.Field activityField =
                            record.getClass().getDeclaredField("activity");
                    activityField.setAccessible(true);
                    Activity activity = (Activity) activityField.get(record);
                    if (activity != null && !activity.isFinishing()) {
                        INSTANCE.activity = activity;
                        return activity;
                    }
                }
            }
        } catch (Throwable t) {
            android.util.Log.w("nsh", "ScannerBridge activity lookup failed", t);
        }
        return null;
    }

    /** Opens the scanner overlay over the current activity. */
    public static void present(Object activityObj, String hint) {
        runOnUiThread(() -> INSTANCE.presentOnUi((Activity) activityObj, hint));
    }

    /** Removes the scanner overlay (cancel, result, or scanner shutdown). */
    public static void dismiss() {
        runOnUiThread(INSTANCE::dismissOnUi);
    }

    private static void runOnUiThread(Runnable work) {
        if (Looper.myLooper() == Looper.getMainLooper()) {
            work.run();
        } else {
            new Handler(Looper.getMainLooper()).post(work);
        }
    }

    // ---- the overlay, on the UI thread ----

    private void presentOnUi(Activity activity, String hint) {
        if (activity.isFinishing() || activity.isDestroyed()) {
            return;
        }
        dismissOnUi();
        this.activity = activity;

        ViewGroup content = activity.findViewById(android.R.id.content);
        overlay = new FrameLayout(activity);
        overlay.setBackgroundColor(Color.BLACK);

        preview = new SurfaceView(activity);
        preview.getHolder().addCallback(this);
        overlay.addView(preview, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));

        TextView label = new TextView(activity);
        label.setText(hint);
        label.setTextColor(Color.WHITE);
        label.setTextSize(15);
        label.setPadding(48, 96, 48, 0);
        overlay.addView(label, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP));

        Button cancel = new Button(activity);
        cancel.setText("取消");
        cancel.setAllCaps(false);
        cancel.setTextColor(Color.WHITE);
        cancel.setBackgroundColor(0x66000000);
        cancel.setOnClickListener(v -> {
            dismissOnUi();
            nativeOnCancel();
        });
        FrameLayout.LayoutParams cancelLayout = new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT,
                Gravity.TOP | Gravity.START);
        cancelLayout.setMargins(40, 180, 0, 0);
        overlay.addView(cancel, cancelLayout);

        content.addView(overlay, new ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
    }

    private void dismissOnUi() {
        if (overlay != null) {
            ViewGroup parent = (ViewGroup) overlay.getParent();
            if (parent != null) {
                parent.removeView(overlay);
            }
            overlay = null;
            preview = null;
        }
    }

    // ---- SurfaceHolder.Callback: the preview surface for the camera ----

    @Override
    public void surfaceCreated(SurfaceHolder holder) {}

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        nativeOnSurface(holder.getSurface(), width, height);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        nativeOnSurfaceLost();
    }

    // ---- Application.ActivityLifecycleCallbacks ----

    @Override
    public void onActivityResumed(Activity a) {
        activity = a;
    }

    @Override
    public void onActivityPaused(Activity a) {
        if (a == activity) {
            activity = null;
        }
    }

    @Override
    public void onActivityCreated(Activity a, Bundle state) {}

    @Override
    public void onActivityStarted(Activity a) {}

    @Override
    public void onActivityStopped(Activity a) {}

    @Override
    public void onActivitySaveInstanceState(Activity a, Bundle state) {}

    @Override
    public void onActivityDestroyed(Activity a) {}

    // ---- bound from Rust with RegisterNatives ----

    private static native void nativeOnSurface(Surface surface, int width, int height);

    private static native void nativeOnSurfaceLost();

    private static native void nativeOnCancel();
}
