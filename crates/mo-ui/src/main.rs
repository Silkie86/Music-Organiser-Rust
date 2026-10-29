// No console window behind the app on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use mo_app::places::Place;
use mo_app::shortcuts::Shortcuts;
use mo_app::themes::Look;
use slint::ComponentHandle;

fn main() -> Result<(), slint::PlatformError> {
    // The settings come from library.db once mo-library exists (phase 2); until then the app opens in its defaults.
    let app = mo_ui::new_app(&Look::from_settings(None, None), Shortcuts::from_setting(None), Place::Songs)?;
    app.window().set_maximized(true);
    app.run()
}
