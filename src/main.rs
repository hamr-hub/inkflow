//! Headless harness + production renderer.
//!
//! Harness modes: `--headless`, `--layout-test`, `--compose-test`,
//! `--works-gallery`, and `--drm-test` (real display live loop). CLI parsing
//! lives in [`crate::cli`]; this one is purely orchestration.

mod cli;
mod harness;

fn main() -> std::process::ExitCode {
    let args = cli::parse();
    match args.mode {
        cli::Mode::Help => {
            cli::print_help();
            return std::process::ExitCode::SUCCESS;
        }
        cli::Mode::BadFlag => {
            cli::print_help();
            return std::process::ExitCode::from(2);
        }
        _ => {}
    }

    // Wall-clock anchor for the telemetry log.
    let t0 = std::time::Instant::now();
    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    eprintln!(
        "inkflow: mode={:?} size={}x{} frames={} out={}",
        args.mode, args.width, args.height, args.frames, args.out_dir
    );

    match args.mode {
        cli::Mode::Headless => {
            let _ = std::fs::create_dir_all(&args.out_dir);
            harness::capture::run_headless(
                args.width,
                args.height,
                args.frames,
                &args.out_dir,
                t0,
                started_ms,
            );
            std::process::ExitCode::SUCCESS
        }
        cli::Mode::LayoutTest => {
            let _ = std::fs::create_dir_all(&args.out_dir);
            harness::capture::run_capture(
                args.width,
                args.height,
                &harness::capture::Capture {
                    prefix: "layout",
                    label: "layout suite",
                    dt: 0.10,
                    frames: args.frames.max(1),
                    // Mid-sequence, so the accent is framed by settled content
                    // on both sides.
                    touch_at: args.frames / 2,
                    touch: (0.5, 0.55, 0.9),
                    marked_slots: false,
                },
                &args.out_dir,
            );
            std::process::ExitCode::SUCCESS
        }
        cli::Mode::ComposeTest => {
            let _ = std::fs::create_dir_all(&args.out_dir);
            let frames = args.frames.max(1);
            harness::capture::run_capture(
                args.width,
                args.height,
                &harness::capture::Capture {
                    prefix: "compose",
                    label: "compose suite",
                    // Slower dt so entrance + hold + touch + refresh all land
                    // inside one captured run. 0.4 s/step × 12 frames ≈ 4.8 s,
                    // covering one hero beat plus a second entrance.
                    dt: 0.40,
                    frames,
                    // Near the end, so the touch frame is meaningful.
                    touch_at: frames.saturating_sub(3).max(2),
                    // Upper-right, where the supporting echo sits.
                    touch: (0.86, 0.18, 0.9),
                    marked_slots: true,
                },
                &args.out_dir,
            );
            std::process::ExitCode::SUCCESS
        }
        cli::Mode::WorksGallery => {
            let _ = std::fs::create_dir_all(&args.out_dir);
            harness::gallery::run(args.width, args.height, &args.out_dir);
            std::process::ExitCode::SUCCESS
        }
        cli::Mode::DrmTest => match harness::live::open_display(args.width, args.height) {
            Some(mut surf) => {
                harness::live::run(&mut surf, t0);
                std::process::ExitCode::SUCCESS
            }
            None => {
                eprintln!("inkflow: --drm-test could not open any display backend; falling back to headless (12 frames).");
                let _ = std::fs::create_dir_all(&args.out_dir);
                harness::capture::run_headless(
                    args.width,
                    args.height,
                    12,
                    &args.out_dir,
                    t0,
                    started_ms,
                );
                std::process::ExitCode::SUCCESS
            }
        },
        // Handled above, before the render paths are entered.
        cli::Mode::Help | cli::Mode::BadFlag => unreachable!(),
    }
}
