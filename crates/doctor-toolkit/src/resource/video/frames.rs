//! Random access to the decoded frames of a movie, for players and exports.
//!
//! [`MovieFrames`] keeps one VP6 decoder: going forward decodes the frames in
//! between (or starts from a key frame when one is closer); going back
//! decodes again from the key frame at or before the wanted frame.

use super::Movie;
use super::container::VideoHeader;
use super::vp6::{Decoder, Picture};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
struct FrameRef {
    start: usize,
    end: usize,
    key: bool,
}

/// The frames of a movie (any byte container: `&[u8]`, `Arc<Vec<u8>>`…),
/// decoded on demand.
pub struct MovieFrames<D: AsRef<[u8]>> {
    data: D,
    frames: Vec<FrameRef>,
    header: VideoHeader,
    decoder: Decoder,
    /// Frame whose picture the decoder holds.
    position: Option<usize>,
    /// Frames decoded since creation (for statistics).
    decoded: u64,
}

impl<D: AsRef<[u8]>> MovieFrames<D> {
    /// Indexes the video frames of an EA movie.
    pub fn new(data: D) -> Result<Self> {
        let (frames, header) = {
            let bytes = data.as_ref();
            let movie = Movie::parse(bytes)?;
            let header = movie
                .video
                .ok_or_else(|| Error::InvalidArgument("the movie has no video".into()))?;
            let base = bytes.as_ptr() as usize;
            let frames: Vec<FrameRef> = movie
                .frames()
                .map(|(key, body)| {
                    let start = body.as_ptr() as usize - base;
                    FrameRef {
                        start,
                        end: start + body.len(),
                        key,
                    }
                })
                .collect();
            (frames, header)
        };
        if frames.is_empty() {
            return Err(Error::InvalidArgument("the movie has no frames".into()));
        }
        Ok(Self {
            data,
            frames,
            header,
            decoder: Decoder::new(),
            position: None,
            decoded: 0,
        })
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn header(&self) -> &VideoHeader {
        &self.header
    }

    /// Frames per second (29.97 when the header has none).
    pub fn fps(&self) -> f64 {
        let fps = self.header.fps();
        if fps.is_finite() && fps > 0.0 {
            fps
        } else {
            30000.0 / 1001.0
        }
    }

    /// Whether the picture is stored bottom-up (`vp60`) and must be flipped.
    pub fn flipped(&self) -> bool {
        self.header.codec.eq_ignore_ascii_case(b"vp60")
    }

    pub fn is_key(&self, index: usize) -> bool {
        self.frames.get(index).is_some_and(|f| f.key)
    }

    /// The key frame at or before `index` (the first frame if there is none).
    pub fn key_at_or_before(&self, index: usize) -> usize {
        let index = index.min(self.frames.len() - 1);
        (0..=index).rev().find(|&i| self.frames[i].key).unwrap_or(0)
    }

    /// The frame the decoder holds.
    pub fn position(&self) -> Option<usize> {
        self.position
    }

    /// Frames decoded so far.
    pub fn decoded(&self) -> u64 {
        self.decoded
    }

    /// Decodes frame `index` (clamped to the last frame) and returns the
    /// coded picture (see [`Self::flipped`] and [`Self::display`]).
    ///
    /// Damaged frames are concealed when possible, as the reference decoder
    /// does; an error is returned only when there is no picture to show.
    pub fn seek(&mut self, index: usize) -> Result<&Picture> {
        let index = index.min(self.frames.len() - 1);
        let key = self.key_at_or_before(index);
        let start = match self.position {
            Some(p) if p == index && self.decoder.picture().is_some() => {
                return Ok(self.decoder.picture().expect("picture"));
            }
            // Forward without a key frame in between: continue.
            Some(p) if p < index && key <= p => p + 1,
            _ => key,
        };
        if start == key && !self.frames[key].key {
            // No key frame before: the first frame is all there is.
            self.decoder.reset();
        }
        for i in start..=index {
            let f = self.frames[i];
            self.decoded += 1;
            let result = self
                .decoder
                .decode(&self.data.as_ref()[f.start..f.end])
                .map(|_| ());
            self.position = Some(i);
            if let Err(e) = result {
                // A frame that could not be concealed leaves the picture
                // before it: fine within a run, not for the first key frame.
                let dropped = !self.decoder.last_damaged();
                if self.decoder.picture().is_none() || (dropped && i == start && start == key) {
                    self.position = None;
                    return Err(e);
                }
            }
        }
        self.decoder
            .picture()
            .ok_or_else(|| Error::malformed("VP6", 0, "no picture"))
    }

    /// The picture of [`Self::position`] in coded orientation.
    pub fn picture(&self) -> Option<&Picture> {
        self.position.and(self.decoder.picture())
    }

    /// Size shown: the header's (the coded picture can be larger).
    pub fn display_size(&self) -> (usize, usize) {
        let (w, h) = (self.header.width as usize, self.header.height as usize);
        match self.decoder.picture() {
            Some(p) if w > 0 && h > 0 && w <= p.width && h <= p.height => (w & !1, h & !1),
            Some(p) => (p.width, p.height),
            None => (w & !1, h & !1),
        }
    }

    /// Frame `index` as shown: upright and cropped to the header's size.
    pub fn display(&mut self, index: usize) -> Result<Picture> {
        let flip = self.flipped();
        let pic = self.seek(index)?;
        let pic = if flip { pic.flipped() } else { pic.clone() };
        let (w, h) = self.display_size();
        Ok(if (w, h) != (pic.width, pic.height) {
            pic.cropped(w, h)
        } else {
            pic
        })
    }

    /// Frame `index` as RGBA pixels (upright, cropped) into `out`; returns
    /// the width and height.
    pub fn rgba(&mut self, index: usize, out: &mut Vec<u8>) -> Result<(usize, usize)> {
        let flip = self.flipped();
        self.seek(index)?;
        let (w, h) = self.display_size();
        let pic = self.decoder.picture().expect("decoded picture");
        pic.rgba_into(w, h, flip, out);
        Ok((w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::super::vp6::{Encoder, EncoderConfig};
    use super::super::{AudioInput, BuildOptions, VideoInput, build_movie, input};
    use super::*;

    /// A moving picture: a ramp sliding right and a square going down.
    fn y4m(w: usize, h: usize, n: usize) -> Vec<u8> {
        let mut d = input::y4m_header(w, h, (30, 1)).into_bytes();
        for t in 0..n {
            d.extend_from_slice(b"FRAME\n");
            for y in 0..h {
                for x in 0..w {
                    let square = (x / 8 == 2) && (y + 40 - t * 2) % 40 < 8;
                    d.push(if square {
                        235
                    } else {
                        ((x * 3 + t * 5) % 200 + 20) as u8
                    });
                }
            }
            d.extend((0..w * h / 2).map(|i| (100 + (i + t) % 50) as u8));
        }
        d
    }

    fn movie(n: usize, keyframe_interval: u32) -> Vec<u8> {
        let src = y4m(48, 32, n);
        let mut reader = &src[..];
        let mut opts = BuildOptions {
            encoder: EncoderConfig {
                keyframe_interval,
                scene_cut: 255,
                ..EncoderConfig::default()
            },
            ..BuildOptions::default()
        };
        build_movie(VideoInput::Y4m(&mut reader), AudioInput::None, &mut opts).unwrap()
    }

    /// Any order of seeks gives the pictures of a straight decode.
    #[test]
    fn seeks_match_sequential_decoding() {
        let data = movie(20, 6);
        let mut straight = MovieFrames::new(&data[..]).unwrap();
        assert_eq!(straight.len(), 20);
        let expected: Vec<Picture> = (0..20).map(|i| straight.display(i).unwrap()).collect();
        assert!(straight.is_key(0) && straight.is_key(6) && !straight.is_key(7));
        assert_eq!(straight.key_at_or_before(11), 6);
        let mut frames = MovieFrames::new(&data[..]).unwrap();
        for &i in &[13, 2, 19, 19, 7, 8, 6, 0, 5, 18, 12, 30] {
            let pic = frames.display(i).unwrap();
            assert_eq!(pic, expected[i.min(19)], "frame {i}");
            assert_eq!(frames.position(), Some(i.min(19)));
        }
        // Forward without a key frame in between decodes only the frames after.
        frames.seek(13).unwrap();
        let before = frames.decoded();
        frames.seek(15).unwrap();
        frames.seek(15).unwrap();
        assert_eq!(frames.decoded(), before + 2);
        // Back: from the key frame at or before.
        frames.seek(14).unwrap();
        assert_eq!(frames.decoded(), before + 2 + 3);
        let mut rgba = Vec::new();
        assert_eq!(frames.rgba(3, &mut rgba).unwrap(), (48, 32));
        assert_eq!(rgba.len(), 48 * 32 * 4);
        assert!(rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    /// Damaged and garbage frames give errors or concealed pictures, never a
    /// panic; decoding goes on at the next frames.
    #[test]
    fn damaged_frames_do_not_stop_decoding() {
        let mut enc = Encoder::new(48, 32, EncoderConfig::default());
        let src = Picture::new(48, 32);
        let key = enc.encode(&src).data;
        let inter = enc.encode(&src).data;
        let mut dec = Decoder::new();
        assert!(dec.decode(&inter).is_err(), "inter frame first");
        dec.decode(&key).unwrap();
        let mut state = 99u32;
        for n in 0..300 {
            let mut frame = if n % 2 == 0 {
                inter.clone()
            } else {
                key.clone()
            };
            let cut = (n * 7) % frame.len().max(1);
            frame.truncate(cut.max(1));
            for b in frame.iter_mut().skip(3) {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
                if state >> 28 == 0 {
                    *b = (state >> 16) as u8;
                }
            }
            let _ = dec.decode(&frame);
        }
        dec.decode(&key).unwrap();
        dec.decode(&inter).unwrap();
        assert!(dec.picture().is_some());
    }
}
