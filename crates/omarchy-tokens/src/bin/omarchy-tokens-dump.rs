//! Prints the resolved Omarchy tokens, and optionally streams changes.
//!
//! Proves the crate against the real system with no UI involved, which is why
//! `omarchy-tokens` has no gpui dependency.
//!
//! ```text
//! omarchy-tokens-dump            # one snapshot
//! omarchy-tokens-dump --watch    # ...then stream every change until Ctrl-C
//! ```
//!
//! With `--watch`, run `omarchy theme set <name>`, `omarchy font set <name>` or
//! `omarchy display text size <n>` in another terminal and the change should
//! appear here within a few hundred milliseconds.

use std::time::Duration;

use anyhow::Result;
use omarchy_tokens::{Paths, Rgb, Tokens};

fn main() -> Result<()> {
    let watching = std::env::args().any(|a| a == "--watch" || a == "-w");

    if !watching {
        print_tokens(&omarchy_tokens::load()?);
        return Ok(());
    }

    let mut watcher = omarchy_tokens::watch()?;
    print_tokens(watcher.current());
    eprintln!(
        "\nwatching {} — Ctrl-C to stop",
        Paths::system().state_current.display()
    );

    loop {
        if watcher.wait(Duration::from_secs(3600)) {
            println!("\n{}", "─".repeat(64));
            print_tokens(watcher.current());
        }
    }
}

fn print_tokens(tokens: &Tokens) {
    let Tokens {
        theme_name,
        palette,
        typography,
        spacing,
        controls,
        surfaces,
        geometry,
        shell,
    } = tokens;

    println!("theme      {theme_name}  ({:?})", palette.mode());
    println!("font       {}", typography.family);
    println!("base-size  {}px", typography.base_size);
    println!(
        "  type     caption {} · body-small {} · body {} · subtitle {} · title {} · heading {} · display {}",
        typography.caption(),
        typography.body_small(),
        typography.body(),
        typography.subtitle(),
        typography.title(),
        typography.heading(),
        typography.display(),
    );
    println!(
        "  spacing  ×{:.3}  xs {} · sm {} · md {} · lg {} · xl {} · row-height {} · panel-pad {}",
        spacing.scale(),
        spacing.xs(),
        spacing.sm(),
        spacing.md(),
        spacing.lg(),
        spacing.xl(),
        spacing.control_height(),
        spacing.panel_padding(),
    );
    println!(
        "  geometry radius {}px · gaps {}px   (from hyprland)",
        geometry.corner_radius, geometry.gaps_out
    );
    println!(
        "  states   normal {:.2} · hover {:.2} · focus {:.2} · selected {:.2} · pressed {:.2}",
        controls.normal.fill_alpha,
        controls.hover_cursor.fill_alpha,
        controls.focus.fill_alpha,
        controls.selected.fill_alpha,
        controls.pressed_fill_alpha,
    );
    println!(
        "  surfaces popup-border {} · tooltip-border {} · menu-border {}",
        surfaces.popups.border.to_hex(),
        surfaces.tooltip.border.to_hex(),
        surfaces.menu.border.to_hex(),
    );
    println!("  shell    {} keys merged", shell.keys().count());
    println!();

    for (name, color) in [
        ("background", palette.background()),
        ("lighter_bg", palette.lighter_background()),
        ("foreground", palette.foreground()),
        ("bright_fg", palette.bright_foreground()),
        ("accent", palette.accent()),
        ("selection", palette.selection()),
        ("muted", palette.muted()),
        ("urgent (red)", palette.urgent()),
        ("green", palette.green()),
        ("yellow", palette.yellow()),
        ("orange", palette.orange()),
        ("blue", palette.blue()),
        ("magenta", palette.magenta()),
        ("brown", palette.brown()),
    ] {
        println!("{}  {:<14} {}", swatch(color), name, color.to_hex());
    }
}

/// A truecolor block, so the palette is verifiable by eye in a terminal rather
/// than only by hex comparison.
fn swatch(c: Rgb) -> String {
    format!("\x1b[48;2;{};{};{}m    \x1b[0m", c.r, c.g, c.b)
}
