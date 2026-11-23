//! Overlay-owned lazy parts session. No work occurs in its constructor.
use crate::capture::Snapshot;
use crate::geometry::{Point, Rect};
use crate::parts::{Mode, Source};
use crate::partworker::{Answer, Query, Request, Worker};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Default)]
pub struct Session {
    pub mode: Mode,
    worker: Option<Worker>,
    target: Option<(isize, Point)>,
    id: u64,
    since: Option<Instant>,
    frozen: Rect,
    query: Option<Query>,
    edge: Option<crate::edges::Split>,
    edges_ms: f64,
    pixels: HashMap<isize, (Rect, Arc<Vec<u8>>)>,
    first_logged: bool,
    first_ms: Option<f64>,
    edges_logged: bool,
}
impl Session {
    pub fn sync(&mut self, snap: &Snapshot, hover: Option<usize>, point: Point, overlay: isize) {
        if !self.mode.active() || hover.is_none() {
            if let Some(worker) = &self.worker {
                worker.cancel();
            }
            self.target = None;
            self.query = None;
            self.edge = None;
            self.mode.window(None);
            return;
        }
        let window = &snap.windows[hover.unwrap()];
        self.mode.window(Some(window.hwnd));
        if self.target == Some((window.hwnd, point)) {
            return;
        }
        let same_window = self.target.is_some_and(|(hwnd, _)| hwnd == window.hwnd);
        if !same_window {
            self.edge = None;
            self.first_logged = false;
            self.first_ms = None;
            self.edges_logged = false;
        }
        self.query = None;
        self.target = Some((window.hwnd, point));
        self.frozen = window.rect;
        self.since = Some(Instant::now());
        self.id += 1;
        if self.worker.is_none() {
            self.worker = Worker::start(overlay).ok();
        }
        // Read back only when parts are explicitly requested. Capture/first-frame is untouched.
        let (image_rect, pixels) = self
            .pixels
            .entry(window.hwnd)
            .or_insert_with(|| {
                let image_rect = snap.layout.clip(&window.rect).unwrap_or_default();
                let data = snap
                    .crop(&image_rect)
                    .ok()
                    .map(|(_, _, p)| p)
                    .unwrap_or_default();
                (image_rect, Arc::new(data))
            })
            .clone();
        if let Some(worker) = &self.worker {
            worker.request(Request {
                hwnd: window.hwnd,
                point,
                frozen: window.rect,
                id: self.id,
                image_rect,
                pixels: Some(pixels),
            });
        }
    }
    pub fn poll(&mut self) {
        if let Some(answer) = self.worker.as_ref().and_then(Worker::take) {
            self.accept(answer);
        }
        let (source, levels) = self.selection();
        if source != Source::None {
            self.mode.resolve(levels.len());
        }
    }
    pub(crate) fn accept(&mut self, answer: Answer) {
        if answer.id != self.id || !self.mode.active() {
            return;
        }
        if let Some(query) = answer.query {
            if !self.first_logged {
                crate::logf!(
                    "parts_on=true uia_first_ms={:.1} uia_levels={}",
                    answer.first_ms.unwrap_or(query.ms),
                    query.levels.len()
                );
                self.first_logged = true;
                self.first_ms = Some(answer.first_ms.unwrap_or(query.ms));
            }
            self.query = Some(query);
        }
        if let Some(edge) = answer.edges {
            self.edge = Some(edge);
            self.edges_ms = answer.edges_ms;
        }
    }
    pub fn selection(&self) -> (Source, Vec<Rect>) {
        let Some((_, point)) = self.target else {
            return (Source::None, Vec::new());
        };
        let edges = self
            .edge
            .as_ref()
            .map(|s| crate::parts::levels(self.frozen, &s.root.chain(point)))
            .unwrap_or_else(|| vec![self.frozen]);
        let source = crate::parts::choose(
            self.query.as_ref().map(|q| q.levels.as_slice()),
            &edges,
            self.since.map(|t| t.elapsed().as_millis()).unwrap_or(0),
        );
        let levels = match source {
            Source::Uia => self.query.as_ref().unwrap().levels.clone(),
            Source::Edges => edges,
            Source::None => vec![self.frozen],
        };
        (source, levels)
    }
    pub fn current(&self) -> Option<Rect> {
        let (source, levels) = self.selection();
        if source == Source::None {
            return None;
        }
        levels.get(self.mode.index(levels.len())).copied()
    }
    pub fn label(&self, lang: crate::i18n::Lang) -> String {
        let (source, levels) = self.selection();
        let tr = lang == crate::i18n::Lang::Tr;
        if source == Source::None {
            return if tr { "parça yok" } else { "no parts" }.into();
        }
        let index = self.mode.index(levels.len()) + 1;
        let mut s = if tr {
            format!("parça {index}/{} · tekerlek büyüt/küçült", levels.len())
        } else {
            format!("part {index}/{} · wheel larger/smaller", levels.len())
        };
        if source == Source::Edges {
            s.push_str(if tr { " · kenar" } else { " · edges" });
        }
        s
    }
    pub fn metadata(&self) -> String {
        let (source, levels) = self.selection();
        format!(
            "parts_on=true source={} level={} uia_ms={:.1} uia_first_ms={:.1} uia_levels={}{}",
            source.name(),
            self.mode.index(levels.len()),
            self.query.as_ref().map(|q| q.ms).unwrap_or(0.),
            self.first_ms.unwrap_or(0.),
            self.query.as_ref().map(|q| q.levels.len()).unwrap_or(0),
            if source == Source::Edges {
                format!(" edges_ms={:.1}", self.edges_ms)
            } else {
                String::new()
            }
        )
    }
    pub fn log_used(&mut self) {
        if !self.edges_logged && self.selection().0 == Source::Edges {
            crate::logf!(
                "parts_on=true edges_ms={:.1} uia_first_ms={:.1} uia_levels={}",
                self.edges_ms,
                self.first_ms.unwrap_or(0.),
                self.query.as_ref().map(|q| q.levels.len()).unwrap_or(0)
            );
            self.edges_logged = true;
        }
    }
    pub(crate) fn has_worker(&self) -> bool {
        self.worker.is_some()
    }
    pub fn stop(&mut self) {
        self.mode.reset();
        if let Some(worker) = &self.worker {
            worker.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_answers_and_first_vs_latest_timing_are_distinct() {
        let whole = Rect::new(0, 0, 1000, 800);
        let pane = Rect::new(0, 0, 400, 800);
        let mut s = Session {
            id: 2,
            frozen: whole,
            target: Some((11, Point::new(100, 100))),
            ..Default::default()
        };
        s.mode.alt(true, true);
        let answer = |id, ms| Answer {
            id,
            query: Some(Query {
                levels: vec![whole, pane],
                ms,
                nodes_seen: 2,
            }),
            first_ms: Some(12.),
            edges: None,
            edges_ms: 0.,
            retry: 0,
        };
        s.accept(answer(1, 99.));
        assert_eq!(s.selection().0, Source::None);
        // The single shared slot may have replaced the first reply with a retry.
        s.accept(answer(2, 5.));
        let text = s.metadata();
        assert!(
            text.contains("source=uia")
                && text.contains("level=1")
                && text.contains("uia_first_ms=12.0")
                && text.contains("uia_ms=5.0")
                && text.contains("uia_levels=2"),
            "{text}"
        );
    }
}
