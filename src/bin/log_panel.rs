//! In-app log capture for the loader's tracing events.
//!
//! Hooks `LogPlugin::custom_layer` to feed a ring buffer that the
//! viewer's "Log" panel displays. Filtered to the bevy_openusd /
//! usd_schemas crates and INFO-and-stricter so the buffer doesn't
//! fill with framework noise.
//!
//! The layer holds an `Arc<Mutex<VecDeque<LogLine>>>`; the same Arc
//! sits inside a Bevy `Resource` (`LoaderLog`) so the UI can read it
//! every frame without going through the tracing API.

use bevy::log::tracing::{self, field, Event, Level, Subscriber};
use bevy::log::tracing_subscriber::{layer::Context, Layer};
use bevy::log::BoxedLayer;
use bevy::prelude::*;
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Ring-buffer cap. Past this we drop the oldest entry per insert.
const MAX_LOG_LINES: usize = 500;

/// Bevy resource — the UI reads `inner.lock()` each frame to render
/// the log panel.
#[derive(Resource, Default, Clone)]
pub struct LoaderLog {
    pub buffer: Arc<Mutex<VecDeque<LogLine>>>,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub level: Level,
    pub target: String,
    pub message: String,
}

/// Tracing field visitor that captures the formatted `message` field
/// into a single string. Falls back to `record_debug` when the field
/// arrives as a `dyn Debug` instead of a `&str`.
struct LogVisitor(String);

impl field::Visit for LogVisitor {
    fn record_str(&mut self, fld: &field::Field, value: &str) {
        if fld.name() == "message" {
            self.0 = value.to_string();
        }
    }
    fn record_debug(&mut self, fld: &field::Field, value: &dyn fmt::Debug) {
        if fld.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

pub struct LoaderLogLayer {
    pub buffer: Arc<Mutex<VecDeque<LogLine>>>,
}

impl<S> Layer<S> for LoaderLogLayer
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let target = event.metadata().target();
        if !(target.starts_with("bevy_openusd") || target.starts_with("usd_schemas")) {
            return;
        }
        let level = *event.metadata().level();
        // tracing `Level` orders ERROR < WARN < INFO < DEBUG < TRACE
        // (more verbose = "greater"), so `> INFO` keeps INFO/WARN/ERROR
        // and drops DEBUG/TRACE. Matches what the panel actually wants.
        if level > Level::INFO {
            return;
        }
        let mut visitor = LogVisitor(String::new());
        event.record(&mut visitor);
        let line = LogLine {
            level,
            target: target.to_string(),
            message: visitor.0,
        };
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push_back(line);
            while buf.len() > MAX_LOG_LINES {
                buf.pop_front();
            }
        }
    }
}

/// `LogPlugin::custom_layer` factory — installs the resource AND
/// returns the boxed layer so the same `Arc` is shared between the
/// tracing subscriber and the UI.
///
/// Must be a free `fn` (no captures) because `LogPlugin` stores it
/// as a function pointer, not a closure.
pub fn loader_log_custom_layer(app: &mut App) -> Option<BoxedLayer> {
    let log = LoaderLog::default();
    let layer = LoaderLogLayer {
        buffer: Arc::clone(&log.buffer),
    };
    app.insert_resource(log);
    Some(Box::new(layer))
}

// Suppress unused-import warnings when only one Level path is used.
#[allow(dead_code)]
const _: () = {
    let _ = tracing::Level::TRACE;
};
