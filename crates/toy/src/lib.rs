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
