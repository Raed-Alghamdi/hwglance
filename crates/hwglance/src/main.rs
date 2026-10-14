use hwglance::app::{App, Mode, Opts};

fn usage() -> ! {
    eprintln!("usage: hwglance [--demo | --mock <snapshot.json>] [--settings] [--focus <reading id>] [--version]");
    std::process::exit(2);
}

fn main() -> eframe::Result {
    hwglance_proto::crash::install("hwglance");
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
    eframe::run_native("hwglance", options, Box::new(|cc| Ok(Box::new(App::new(cc, mode, opts)))))
}
