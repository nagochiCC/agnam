mod app;
mod archive;
mod bookshelf;
mod cache_file;
mod covers;
mod document;
mod error;
mod favorites;
mod history;
mod settings;
mod thumbnail;
mod viewer;

use app::App;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

fn main() {
    let application = adw::Application::builder()
        .application_id("io.github.nagochicc.agnam")
        .build();
    application.connect_startup(|_| {
        adw::init().expect("failed to initialize libadwaita");
    });

    let runtime: Rc<RefCell<Option<app::AppRuntime>>> = Rc::new(RefCell::new(None));
    application.connect_activate({
        let runtime = runtime.clone();
        move |application| {
            if let Some(existing) = runtime.borrow().as_ref() {
                existing.window().present();
                return;
            }

            let launched = App::launch(application);
            let window = launched.window().clone();
            let runtime_weak = Rc::downgrade(&runtime);
            window.connect_close_request(move |_| {
                if let Some(runtime) = runtime_weak.upgrade()
                    && let Some(launched) = runtime.borrow_mut().take()
                {
                    launched.close();
                }
                gtk::glib::Propagation::Proceed
            });
            *runtime.borrow_mut() = Some(launched);
            window.present();
        }
    });
    application.connect_shutdown({
        move |_| {
            if let Some(launched) = runtime.borrow_mut().take() {
                launched.close();
            }
        }
    });

    application.run();
}
