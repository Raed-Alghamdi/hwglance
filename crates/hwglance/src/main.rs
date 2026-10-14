use hwglance::app::{App, Mode, Opts};

fn usage() -> ! {
    eprintln!("usage: hwglance [--demo | --mock <snapshot.json>] [--settings] [--focus <reading id>] [--version]");
    std::process::exit(2);
}

fn main() -> eframe::Result {
    hwglance_proto::crash::install("hwglance");
    if let Err(e) = hwglance::signals::install() {
        eprintln!("hwglance: cannot record termination signals: {e}");
    }
    let mut mode = Mode::Live;
    let mut opts = Opts::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--demo" => mode = Mode::Demo,
            "--mock" => mode = Mode::Mock(args.next().unwrap_or_else(|| usage()).into()),
            "--settings" => opts.open_settings = true,
            "--version" | "-V" => {
                println!("hwglance {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--focus" => opts.focus = Some(args.next().unwrap_or_else(|| usage())),
            _ => usage(),
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("hwglance")
            .with_title("HW Glance")
            .with_inner_size([1500.0, 900.0])
            .with_min_inner_size([800.0, 500.0]),
        ..Default::default()
    };
    log_start();
    let result = eframe::run_native("hwglance", options, Box::new(|cc| Ok(Box::new(App::new(cc, mode, opts)))));
    if let Err(e) = &result {
        // The returned error only reaches stderr, which a launcher discards.
        let (shown, detail) = (e.to_string(), format!("{e:?}"));
        let mut fields = vec![("error", shown.as_str())];
        if detail != shown {
            fields.push(("detail", detail.as_str()));
        }
        hwglance_proto::crash::report("startup or event-loop error", &fields);
    }
    result
}

/// One journal line to tell sessions apart: who started this window.
fn log_start() {
    let parent = std::os::unix::process::parent_id();
    let name = hwglance_proto::crash::process_name(parent).unwrap_or_else(|| "unknown".into());
    hwglance_proto::crash::note(&format!(
        "start: hwglance {} pid {} parent pid {parent} ({name})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    ));
}
