mod app;
mod bridge;
mod catalog;
mod companion;
mod images;
mod models;
mod store;
mod sync;

use android_activity::AndroidApp;
use app::SwayApp;

#[no_mangle]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("Starting Sway Rust (4K OpenGL ES 3.0)...");
    bridge::init(app.vm_as_ptr(), app.activity_as_ptr());

    let mut options = eframe::NativeOptions::default();
    options.android_app = Some(app);

    if let Err(e) = eframe::run_native(
        "Sway Rust",
        options,
        Box::new(|cc| Ok(Box::new(SwayApp::new(cc)))),
    ) {
        log::error!("eframe exited with error: {e:?}");
    }
}
