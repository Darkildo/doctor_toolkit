//! GPU time of the scene pass from timestamp queries (stage 67).
//!
//! Needs [`wgpu::Features::TIMESTAMP_QUERY`] on the device (requested only
//! by the profiler and the benchmark). A frame writes two timestamps
//! around its pass, resolves them and maps a small buffer; the time shows
//! up in a later frame, once the device was polled. Frames rendered while
//! a readback is still mapping are not timed.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use eframe::wgpu;

/// No readback in flight / mapping / mapped and readable.
const IDLE: u8 = 0;
const MAPPING: u8 = 1;
const MAPPED: u8 = 2;

pub struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    /// Nanoseconds per tick.
    period: f32,
    state: Arc<AtomicU8>,
    /// Whether the frame being recorded writes timestamps.
    armed: bool,
    pub last: Option<Duration>,
}

impl GpuTimer {
    /// `None` when the device cannot time passes.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("scene pass time"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(Self {
            queries,
            resolve: buffer(
                "timestamps",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "timestamp readback",
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            period: queue.get_timestamp_period(),
            state: Arc::new(AtomicU8::new(IDLE)),
            armed: false,
            last: None,
        })
    }

    /// Reads a finished measurement and returns the timestamp writes of the
    /// next pass, if this frame can be timed.
    pub fn begin(&mut self) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        if self.state.load(Ordering::Acquire) == MAPPED {
            if let Ok(bytes) = self.readback.slice(..).get_mapped_range() {
                let t0 = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
                let t1 = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
                let ns = t1.saturating_sub(t0) as f64 * self.period as f64;
                self.last = Some(Duration::from_nanos(ns as u64));
            }
            self.readback.unmap();
            self.state.store(IDLE, Ordering::Release);
        }
        self.armed = self.state.load(Ordering::Acquire) == IDLE;
        self.armed.then_some(wgpu::RenderPassTimestampWrites {
            query_set: &self.queries,
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: Some(1),
        })
    }

    /// Copies the timestamps of the pass for reading (before the submit).
    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.armed {
            encoder.resolve_query_set(&self.queries, 0..2, &self.resolve, 0);
            encoder.copy_buffer_to_buffer(&self.resolve, 0, &self.readback, 0, 16);
        }
    }

    /// Starts reading them back (after the submit).
    pub fn submitted(&mut self) {
        if !std::mem::take(&mut self.armed) {
            return;
        }
        self.state.store(MAPPING, Ordering::Release);
        let state = self.state.clone();
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(
                    if result.is_ok() { MAPPED } else { IDLE },
                    Ordering::Release,
                );
            });
    }
}
