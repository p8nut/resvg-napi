//! A tiny library for `napi-gen` to wrap. `crates/toy-napi` binds it with the
//! same generator as the root crate, so the generic half is tested on a crate
//! it was not written for.

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub mod alt;

/// Another crate's enum, re-exported: `toy::Shade` to a caller.
pub use toy_dep::Shade;

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
    origins: Vec<Origin>,
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

    /// Each name with its place: a list of pairs.
    pub fn pairs(&self) -> Vec<(String, u32)> {
        self.names
            .iter()
            .zip(0..)
            .map(|(n, i)| (n.clone(), i))
            .collect()
    }

    /// The key of the name at `place`: an opaque value, passed back to
    /// [`Tally::named`].
    pub fn key(&self, place: u32) -> Option<Key> {
        ((place as usize) < self.names.len()).then_some(Key(place))
    }

    pub fn named(&self, key: Key) -> Option<&str> {
        self.names.get(key.0 as usize).map(String::as_str)
    }

    /// What a tag names: its own name, or the first one here for `Any`.
    pub fn tag_name<'a>(&'a self, tag: &'a Tag) -> &'a str {
        match tag {
            Tag::Named(n) => n,
            Tag::Any => self.names.first().map_or("", String::as_str),
        }
    }

    /// The key of the first name a filter accepts: borrowed lists in, as
    /// a query is.
    pub fn find(&self, filter: &Filter) -> Option<Key> {
        let place = self.names.iter().position(|n| {
            n.len() >= filter.min_len.0 as usize
                && filter
                    .tags
                    .iter()
                    .any(|t| matches!(t, Tag::Any) || *t == Tag::Named(n))
        })?;
        Some(Key(place as u32))
    }

    /// Keeps where some bytes come from, under a key of its own (keys of
    /// origins count apart from names'): a few keys, held inline.
    pub fn stamp(&mut self, origin: Origin) -> ArrayVec<[Key; 4]> {
        self.origins.push(origin);
        let key = Key(self.origins.len() as u32 - 1);
        ArrayVec {
            items: [key; 4],
            len: 1,
        }
    }

    /// Keeps `bytes` pinned to `path`: the origin no JS value builds, on
    /// the targets that have it.
    #[cfg(unix)]
    pub fn pin(&mut self, path: &str, bytes: &[u8]) -> Key {
        self.origins
            .push(Origin::Pinned(path.into(), Arc::new(bytes.to_vec())));
        Key(self.origins.len() as u32 - 1)
    }

    /// The origin under `key` and how many bytes it holds, if any.
    pub fn origin(&self, key: Key) -> Option<(Origin, u32)> {
        let o = self.origins.get(key.0 as usize)?.clone();
        let len = match &o {
            Origin::Inline(b) => b.as_ref().as_ref().len(),
            Origin::Path(_) => 0,
            #[cfg(unix)]
            Origin::Pinned(_, b) => b.as_ref().as_ref().len(),
        };
        Some((o, len as u32))
    }

    /// The first two keys, in a `Cell`: an array in a generic that is no
    /// inline vector, which no binding lists.
    pub fn first_keys(&self) -> std::cell::Cell<[Key; 2]> {
        std::cell::Cell::new([Key(0), Key(1)])
    }

    /// What a hue names: its own name, or the first one here for `Any`.
    /// The hue is the `alt` module's, which takes it nowhere itself.
    pub fn hue_name(&self, hue: &alt::Hue) -> String {
        match hue {
            alt::Hue::Named(n) => n.clone(),
            alt::Hue::Any => self.names.first().cloned().unwrap_or_default(),
        }
    }

    /// The origins kept, in order.
    pub fn origins(&self) -> &[Origin] {
        &self.origins
    }

    /// Runs `p` with the joined names' bytes and how many there are: the
    /// data is lent, not returned.
    pub fn with_joined<P, T>(&self, p: P) -> T
    where
        P: FnOnce(&[u8], u32) -> T,
    {
        p(self.joined().as_bytes(), self.names.len() as u32)
    }
}

/// Where some bytes come from: shared bytes, a path, or both. The last is
/// built on some targets only, as a crate gates a variant on a feature.
#[derive(Clone)]
pub enum Origin {
    Inline(Arc<dyn AsRef<[u8]> + Send + Sync>),
    Path(std::path::PathBuf),
    #[cfg(unix)]
    Pinned(std::path::PathBuf, Arc<dyn AsRef<[u8]> + Send + Sync>),
}

impl fmt::Debug for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Inline(_) => f.write_str("Inline"),
            Origin::Path(p) => write!(f, "Path({p:?})"),
            #[cfg(unix)]
            Origin::Pinned(p, _) => write!(f, "Pinned({p:?})"),
        }
    }
}

/// A canvas with a weight: a class beside a number, which no union holds
/// (a variant with several values is an object, whose fields must be
/// `Clone`, and a class is not).
#[derive(Clone, Debug)]
pub enum Weighted {
    Nothing,
    Held(Canvas, u32),
}

/// A few values held inline, up to the array's length, as a small-vector
/// crate holds them: a list to JS. Named as tinyvec's, the generator knowing
/// inline vectors by name.
pub struct ArrayVec<A> {
    items: A,
    len: usize,
}

impl<T, const N: usize> IntoIterator for ArrayVec<[T; N]> {
    type Item = T;
    type IntoIter = std::iter::Take<std::array::IntoIter<T, N>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().take(self.len)
    }
}

/// A label a step may be checked against: an input enum's shape, but taken
/// only by a method of an enum, which no binding emits.
#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    Unnamed,
    Text(String),
}

/// An opaque key: no public field, `Copy`, printed as a number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key(u32);

impl Key {
    pub fn first() -> Self {
        Key(0)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A tag a filter matches on: a name it borrows, or any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tag<'a> {
    Named(&'a str),
    Any,
}

/// A shortest length, as a number newtype.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MinLen(pub u16);

/// What [`Tally::find`] looks for: a list it borrows.
#[derive(Clone, Copy, Debug, Default)]
pub struct Filter<'a> {
    pub tags: &'a [Tag<'a>],
    pub min_len: MinLen,
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

/// Why a [`Vec2`], a [`Canvas`] or an [`alt::Color`] did not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// Nothing to parse.
    Empty,
    /// A part that is not a number.
    BadNumber(String),
    /// A part that is missing.
    Missing { part: &'static str },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Empty => write!(f, "nothing to parse"),
            ParseError::BadNumber(s) => write!(f, "not a number: {s:?}"),
            ParseError::Missing { part } => write!(f, "missing {part}"),
        }
    }
}

/// One number of a separated list.
fn number<T: FromStr>(part: Option<&str>, name: &'static str) -> Result<T, ParseError> {
    let part = part.ok_or(ParseError::Missing { part: name })?.trim();
    part.parse()
        .map_err(|_| ParseError::BadNumber(part.to_string()))
}

/// A vector in the plane: plain data, `Copy`, parsed from `"x,y"` and
/// printed back the same way.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }

    pub fn zero() -> Self {
        Vec2::default()
    }

    pub fn length(&self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn scaled(&self, k: f32) -> Self {
        Vec2::new(self.x * k, self.y * k)
    }

    pub fn plus(&self, other: Vec2) -> Self {
        Vec2::new(self.x + other.x, self.y + other.y)
    }

    /// `None` for the zero vector, which has no direction.
    pub fn normalized(&self) -> Option<Self> {
        let l = self.length();
        (l > 0.0).then(|| self.scaled(1.0 / l))
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }

    /// `(x, y)`, a pair.
    pub fn parts(&self) -> (f32, f32) {
        (self.x, self.y)
    }
}

impl FromStr for Vec2 {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, ParseError> {
        if s.trim().is_empty() {
            return Err(ParseError::Empty);
        }
        let mut parts = s.split(',');
        Ok(Vec2::new(
            number(parts.next(), "x")?,
            number(parts.next(), "y")?,
        ))
    }
}

impl fmt::Display for Vec2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{}", self.x, self.y)
    }
}

/// A colour whose channels are private: a class with accessors, not plain
/// data. [`alt::Color`] has its name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    r: u8,
    g: u8,
    b: u8,
}

impl Color {
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b }
    }

    /// `None` when a channel is over 255.
    pub fn checked(r: u32, g: u32, b: u32) -> Option<Self> {
        Some(Color::from_rgb(
            r.try_into().ok()?,
            g.try_into().ok()?,
            b.try_into().ok()?,
        ))
    }

    pub fn red(&self) -> u8 {
        self.r
    }

    pub fn set_red(&mut self, r: u8) {
        self.r = r;
    }

    pub fn is_gray(&self) -> bool {
        self.r == self.g && self.g == self.b
    }
}

/// A grid of bytes, owned and written in place.
#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    width: u32,
    data: Vec<u8>,
}

impl Canvas {
    /// `None` when a side is zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let len = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
        Some(Canvas {
            width,
            data: vec![0; len],
        })
    }

    /// From `"<width>x<height>"`.
    pub fn from_text(text: &str) -> Result<Self, ParseError> {
        if text.trim().is_empty() {
            return Err(ParseError::Empty);
        }
        let mut parts = text.split('x');
        let (w, h) = (
            number(parts.next(), "width")?,
            number(parts.next(), "height")?,
        );
        Canvas::new(w, h).ok_or(ParseError::BadNumber(text.to_string()))
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        u32::try_from(self.data.len()).unwrap_or(u32::MAX) / self.width
    }

    /// Sets every byte to the colour's red channel.
    pub fn fill(&mut self, color: Color) {
        self.data.fill(color.r);
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// A copy of the canvas in a frame of its own: a union made, not
    /// borrowed from anything.
    pub fn framed(&self) -> Content {
        Content::Frame(Frame {
            layer: Layer::new("canvas", Vec::new()),
            shades: Vec::new(),
            content: Box::new(Content::Canvas(self.clone())),
        })
    }

    /// A writable view of the bytes, for [`paint`].
    pub fn as_view(&mut self) -> CanvasView<'_> {
        CanvasView {
            width: self.width,
            data: &mut self.data,
        }
    }

    /// The bytes, the canvas consumed.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
}

/// A canvas borrowed for writing.
pub struct CanvasView<'a> {
    width: u32,
    data: &'a mut [u8],
}

/// Why [`paint`] refused.
#[derive(Clone, Debug, PartialEq)]
pub enum PaintError {
    /// The point is off the canvas.
    OutOfBounds(f32, f32),
}

impl fmt::Display for PaintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let PaintError::OutOfBounds(x, y) = self;
        write!(f, "{x},{y} is off the canvas")
    }
}

/// Paints the byte at `at` with the colour's red channel, and returns its
/// index.
pub fn paint(view: &mut CanvasView<'_>, at: Vec2, color: Color) -> Result<u32, PaintError> {
    let off = PaintError::OutOfBounds(at.x, at.y);
    if !(at.x >= 0.0 && at.y >= 0.0) {
        return Err(off);
    }
    let width = f64::from(view.width);
    let (x, y) = (f64::from(at.x).floor(), f64::from(at.y).floor());
    let i = y * width + x;
    if x >= width || i >= view.data.len() as f64 {
        return Err(off);
    }
    let i = i as usize;
    view.data[i] = color.r;
    u32::try_from(i).map_err(|_| off)
}

/// A tree of layers and dots, shared behind an `Arc`: its nodes are only
/// ever borrowed from it, the shape a binding projects.
#[derive(Debug)]
pub struct Scene {
    root: Layer,
    /// Dots shared by name, a table of `Arc`s beside the tree.
    marks: Vec<Arc<Dot>>,
    trail: Trail,
    contents: Vec<Content>,
    frames: Vec<Frame>,
    album: Arc<Album>,
}

impl Scene {
    /// `root` holding the dot `a`, the layer `inner` (dots `b` and `c`, and
    /// the empty layer `empty`), and the dot `d`; and the mark `x`.
    pub fn demo() -> Arc<Scene> {
        let dot = |name: &str, x: f32| {
            Item::Dot(Dot {
                name: name.into(),
                at: Vec2::new(x, 0.0),
            })
        };
        let layer = |name: &str, items| Item::Layer(Box::new(Layer::new(name, items)));
        let frame = Frame {
            layer: Layer::new("framed", vec![dot("f", 5.0)]),
            shades: vec![Shade::Plain, Shade::Named("teal".into())],
            content: Box::new(Content::Layer(Layer::new("inset", vec![dot("i", 7.0)]))),
        };
        Arc::new(Scene {
            root: Layer {
                name: "root".into(),
                copies: Copies,
                items: vec![
                    dot("a", 1.0),
                    layer(
                        "inner",
                        vec![dot("b", 2.0), dot("c", 3.0), layer("empty", vec![])],
                    ),
                    dot("d", 4.0),
                ],
            },
            marks: vec![Arc::new(Dot {
                name: "x".into(),
                at: Vec2::new(9.0, 9.0),
            })],
            trail: Trail {
                steps: vec![
                    Step::Jump(Vec2::new(1.0, 0.0)),
                    Step::Line(Vec2::new(1.0, 0.0), Vec2::new(4.0, 0.0)),
                    Step::Stop,
                ],
            },
            contents: vec![
                Content::Empty,
                Content::Canvas(Canvas::new(2, 1).expect("a 2x1 canvas")),
                Content::Frame(frame.clone()),
                Content::Layer(Layer::new("held", vec![dot("h", 6.0)])),
            ],
            frames: vec![frame.clone()],
            album: Arc::new(Album {
                frames: vec![frame],
            }),
        })
    }

    pub fn root(&self) -> &Layer {
        &self.root
    }

    /// The shared dots: the mark `x`.
    pub fn marks(&self) -> &[Arc<Dot>] {
        &self.marks
    }

    /// The first item called `name`, depth first.
    pub fn find(&self, name: &str) -> Option<&Item> {
        self.root.find(name)
    }

    /// The way through the scene: a jump to `a`, a line to `d`, a stop.
    pub fn trail(&self) -> &Trail {
        &self.trail
    }

    /// What the scene's slots hold: nothing, a 2x1 canvas, a frame around
    /// the layer `framed` (the dot `f`), and the layer `held` (the dot `h`).
    pub fn contents(&self) -> &[Content] {
        &self.contents
    }

    /// The frames, as a table of values: the one around `framed`.
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The same frames, behind an `Arc` of their own.
    pub fn album(&self) -> &Arc<Album> {
        &self.album
    }
}

/// Frames shared behind an `Arc`: a type no binding lists, handed out as
/// the read-only handle its `Arc` makes it.
#[derive(Debug)]
pub struct Album {
    frames: Vec<Frame>,
}

impl Album {
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    /// The first frame.
    pub fn cover(&self) -> &Frame {
        &self.frames[0]
    }
}

/// A slot's content: a variant may carry a class JS can write (`Canvas`),
/// a value that only partly maps (`Frame`), or a projected class (`Layer`).
#[derive(Clone, Debug)]
pub enum Content {
    Empty,
    Canvas(Canvas),
    Frame(Frame),
    Layer(Layer),
}

/// A layer, framed: its members borrow a projected class, a re-exported
/// enum, and a union.
#[derive(Clone, Debug)]
pub struct Frame {
    layer: Layer,
    shades: Vec<Shade>,
    content: Box<Content>,
}

impl Frame {
    pub fn layer(&self) -> &Layer {
        &self.layer
    }

    pub fn shades(&self) -> &[Shade] {
        &self.shades
    }

    /// What the frame holds: the layer `inset` (the dot `i`).
    pub fn content(&self) -> &Content {
        &self.content
    }
}

/// One move of a [`Trail`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Jump(Vec2),
    Line(Vec2, Vec2),
    Stop,
}

impl Step {
    /// Whether this step is the one `label` names: only `Stop` is labelled.
    pub fn labelled(&self, label: &Label) -> bool {
        matches!((self, label), (Step::Stop, Label::Unnamed))
    }
}

/// Steps held as a stream, not a list: its content is what
/// [`Trail::steps`] iterates, the shape a binding hands out as an array.
#[derive(Clone, Debug)]
pub struct Trail {
    steps: Vec<Step>,
}

impl Trail {
    pub fn steps(&self) -> impl Iterator<Item = Step> + '_ {
        self.steps.iter().copied()
    }
}

/// One node of a [`Scene`].
#[derive(Clone, Debug)]
pub enum Item {
    Layer(Box<Layer>),
    Dot(Dot),
}

impl Item {
    pub fn name(&self) -> &str {
        match self {
            Item::Layer(l) => &l.name,
            Item::Dot(d) => &d.name,
        }
    }
}

/// A named list of items.
#[derive(Clone, Debug)]
pub struct Layer {
    name: String,
    items: Vec<Item>,
    // Never read: copying it is what counts.
    #[allow(dead_code)]
    copies: Copies,
}

impl Layer {
    fn new(name: &str, items: Vec<Item>) -> Self {
        Layer {
            name: name.into(),
            items,
            copies: Copies,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    /// The first item, if any.
    pub fn first(&self) -> Option<&Item> {
        self.items.first()
    }

    fn find(&self, name: &str) -> Option<&Item> {
        self.items.iter().find_map(|i| match i {
            _ if i.name() == name => Some(i),
            Item::Layer(l) => l.find(name),
            Item::Dot(_) => None,
        })
    }
}

/// Counts the copies of what holds it, process-wide: [`copies`] reads the
/// count. A binding that borrows a value from its owner makes none.
#[derive(Debug)]
struct Copies;

static COPIES: AtomicU32 = AtomicU32::new(0);

impl Clone for Copies {
    fn clone(&self) -> Self {
        COPIES.fetch_add(1, Ordering::Relaxed);
        Copies
    }
}

/// How many times a [`Layer`] was copied so far.
pub fn copies() -> u32 {
    COPIES.load(Ordering::Relaxed)
}

/// A named point.
#[derive(Clone, Debug)]
pub struct Dot {
    name: String,
    at: Vec2,
}

impl Dot {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn at(&self) -> Vec2 {
        self.at
    }
}

/// How many dots `item` holds, itself included.
pub fn count(item: &Item) -> u32 {
    match item {
        Item::Dot(_) => 1,
        Item::Layer(l) => l.items.iter().map(count).sum(),
    }
}

/// `None` for an empty layer: nothing to visit.
pub fn visit(item: &Item) -> Option<()> {
    match item {
        Item::Layer(l) if l.items.is_empty() => None,
        _ => Some(()),
    }
}

/// Mixes `rounds` times: a call slow enough to keep a worker thread busy
/// while another waits behind it.
pub fn spin(rounds: u32) -> u32 {
    let mut x: u32 = 0x9e37_79b9;
    for i in 0..rounds {
        x = std::hint::black_box(x.rotate_left(5) ^ i).wrapping_mul(0x0100_0193);
    }
    x
}
