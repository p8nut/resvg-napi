//! A tiny library for `napi-gen` to wrap. `crates/toy-napi` binds it with the
//! same generator as the root crate, so the generic half is tested on a crate
//! it was not written for.

use std::sync::Arc;

/// Which way a counter counts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Direction {
    #[default]
    Up,
    Down,
}

/// A point in the plane.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// How to build a [`Counter`].
#[derive(Clone, Debug)]
pub struct Options {
    /// What the counter is called.
    pub label: String,
    /// How far one tick moves it.
    pub step: f32,
    pub direction: Direction,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            label: "counter".into(),
            step: 1.0,
            direction: Direction::Up,
        }
    }
}

/// An immutable counter, shared behind an `Arc`.
#[derive(Debug)]
pub struct Counter {
    options: Options,
    origin: Point,
}

impl Counter {
    pub fn new(options: &Options) -> Arc<Counter> {
        Arc::new(Counter {
            options: options.clone(),
            origin: Point::default(),
        })
    }

    pub fn label(&self) -> &str {
        &self.options.label
    }

    pub fn step(&self) -> f32 {
        match self.options.direction {
            Direction::Up => self.options.step,
            Direction::Down => -self.options.step,
        }
    }

    pub fn direction(&self) -> Direction {
        self.options.direction
    }

    pub fn origin(&self) -> &Point {
        &self.origin
    }
}

/// Names counted so far. Unlike [`Counter`] it is `Clone` and changes in
/// place, which is the shape a binding copies on write.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    names: Vec<String>,
}

impl Tally {
    pub fn new() -> Arc<Tally> {
        Arc::new(Tally::default())
    }

    pub fn push<S: Into<String>>(&mut self, name: S) {
        self.names.push(name.into());
    }

    pub fn clear(&mut self) {
        self.names.clear();
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn joined(&self) -> String {
        self.names.join(",")
    }
}

/// The styles a report can take, indexed by [`ReportOptions::style_index`].
pub static STYLES: &[&str] = &["plain", "loud", "quiet"];

/// How wide a report line is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Width {
    /// As long as it needs.
    #[default]
    Auto,
    /// Padded with dots to this many characters.
    Fixed(u8),
}

/// How [`report`] writes its line.
#[derive(Clone, Debug)]
pub struct ReportOptions {
    /// What the line starts with.
    ///
    /// Default: `tally`
    pub title: String,
    /// Which of [`STYLES`] to use. Indexed without a bounds check.
    ///
    /// Default: 0
    pub style_index: u8,
    /// Names counted before the tally's own.
    ///
    /// Default: none
    pub base: Arc<Tally>,
    /// Default: [`Width::Auto`]
    pub width: Width,
    /// Applied to the finished line.
    ///
    /// Default: unchanged
    pub finish: fn(String) -> String,
}

impl Default for ReportOptions {
    fn default() -> Self {
        ReportOptions {
            title: "tally".into(),
            style_index: 0,
            base: Arc::new(Tally::default()),
            width: Width::Auto,
            finish: |s| s,
        }
    }
}

/// One line about `tally`: `title[style]: base names,tally names`.
pub fn report(tally: &Tally, options: &ReportOptions) -> String {
    let names: Vec<&str> = options
        .base
        .names
        .iter()
        .chain(&tally.names)
        .map(String::as_str)
        .collect();
    let style = STYLES[usize::from(options.style_index)];
    let mut line = format!("{}[{style}]: {}", options.title, names.join(","));
    if let Width::Fixed(n) = options.width {
        while line.len() < usize::from(n) {
            line.push('.');
        }
    }
    (options.finish)(line)
}
