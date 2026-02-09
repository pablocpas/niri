#[macro_use]
extern crate tracing;

use std::cell::Cell;
use std::env;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::{AdwApplicationWindowExt, NavigationPageExt};
use clap::Parser;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use smithay_view::SmithayView;
use tracing_subscriber::EnvFilter;

use crate::cases::all_test_cases;

mod cases;
mod smithay_view;
mod test_window;

#[derive(Clone, Debug, Parser)]
#[command(about = "Run and inspect tiri visual test cases")]
struct Cli {
    /// Run every visual case in sequence and exit (CI smoke mode).
    #[arg(long)]
    smoke_test: bool,

    /// Time spent in each case while running with --smoke-test.
    #[arg(long, default_value_t = 250, value_name = "MS")]
    smoke_test_case_ms: u64,
}

fn main() -> glib::ExitCode {
    let cli = Cli::parse();

    let directives =
        env::var("RUST_LOG").unwrap_or_else(|_| "tiri-visual-tests=debug,tiri=debug".to_owned());
    let env_filter = EnvFilter::builder().parse_lossy(directives);
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(env_filter)
        .init();

    let app = adw::Application::new(None::<&str>, gio::ApplicationFlags::NON_UNIQUE);
    app.connect_startup(on_startup);
    app.connect_activate(move |app| build_ui(app, cli.clone()));
    app.run_with_args::<&str>(&[])
}

fn on_startup(_app: &adw::Application) {
    // Load our CSS.
    let provider = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../resources/style.css"));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_ui(app: &adw::Application, cli: Cli) {
    let stack = gtk::Stack::new();
    let anim_adjustment = gtk::Adjustment::new(1., 0., 10., 0.1, 0.5, 0.);
    let mut smoke_case_ids = Vec::new();
    for case in all_test_cases() {
        let view = SmithayView::new_dyn(case.make, &anim_adjustment);
        stack.add_titled(&view, Some(case.id), case.title);
        smoke_case_ids.push(case.id);
    }

    let content_headerbar = adw::HeaderBar::new();

    let anim_scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&anim_adjustment));
    anim_scale.set_hexpand(true);

    let anim_control_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    anim_control_bar.add_css_class("anim-control-bar");
    anim_control_bar.append(&gtk::Label::new(Some("Slowdown")));
    anim_control_bar.append(&anim_scale);

    let content_view = adw::ToolbarView::new();
    content_view.set_top_bar_style(adw::ToolbarStyle::RaisedBorder);
    content_view.set_bottom_bar_style(adw::ToolbarStyle::RaisedBorder);
    content_view.add_top_bar(&content_headerbar);
    content_view.add_bottom_bar(&anim_control_bar);
    content_view.set_content(Some(&stack));
    let content = adw::NavigationPage::new(
        &content_view,
        stack
            .page(&stack.visible_child().unwrap())
            .title()
            .as_deref()
            .unwrap(),
    );

    let sidebar_header = adw::HeaderBar::new();
    let stack_sidebar = gtk::StackSidebar::new();
    stack_sidebar.set_stack(&stack);
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&stack_sidebar));
    let sidebar = adw::NavigationPage::new(&sidebar_view, "Tests");

    let split_view = adw::NavigationSplitView::new();
    split_view.set_content(Some(&content));
    split_view.set_sidebar(Some(&sidebar));

    stack.connect_visible_child_notify(move |stack| {
        content.set_title(
            stack
                .visible_child()
                .and_then(|c| stack.page(&c).title())
                .as_deref()
                .unwrap_or_default(),
        )
    });

    let window = adw::ApplicationWindow::new(app);
    window.set_title(Some("tiri visual tests"));
    window.set_content(Some(&split_view));
    window.present();

    if cli.smoke_test {
        start_smoke_test(app, &stack, smoke_case_ids, cli.smoke_test_case_ms);
    }
}

fn start_smoke_test(
    app: &adw::Application,
    stack: &gtk::Stack,
    case_ids: Vec<&'static str>,
    case_ms: u64,
) {
    if case_ids.is_empty() {
        warn!("no visual test cases registered");
        app.quit();
        return;
    }

    let step = Duration::from_millis(case_ms.max(50));
    info!(
        "running visual smoke test mode over {} cases, {} ms each",
        case_ids.len(),
        step.as_millis()
    );

    stack.set_visible_child_name(case_ids[0]);
    stack.queue_draw();

    let app = app.clone();
    let stack = stack.clone();
    let case_ids = Rc::new(case_ids);
    let idx = Rc::new(Cell::new(1usize));

    glib::timeout_add_local(step, move || {
        let i = idx.get();
        if i >= case_ids.len() {
            info!("visual smoke test mode completed");
            app.quit();
            return glib::ControlFlow::Break;
        }

        let case_id = case_ids[i];
        debug!("smoke visual case {}/{}: {case_id}", i + 1, case_ids.len());
        stack.set_visible_child_name(case_id);
        stack.queue_draw();
        idx.set(i + 1);

        glib::ControlFlow::Continue
    });
}
