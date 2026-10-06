//! PipeWire backend: capture from a sink's monitor and play to an output node, each on its own thread.
//!
//! Both streams use `RT_PROCESS`; the callbacks only touch the lock-free delay line and analysis ring.
//! Safety for the dev machine: streams target explicit nodes with `node.dont-reconnect` and
//! `node.dont-fallback`, so they never move to the default devices.

use std::sync::mpsc;
use std::thread::JoinHandle;

use anyhow::{Context, Result, anyhow, bail};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::Pod;

use super::{CHANNELS, CaptureProcessor, DelayReader, SAMPLE_RATE};

/// Requested quantum: 256 frames (5.3 ms).
const LATENCY: &str = "256/48000";

/// A running stream thread; dropping it (or calling [`PwHandle::stop`]) stops the stream.
pub struct PwHandle {
    sender: Option<pw::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl PwHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        if let Some(tx) = self.sender.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PwHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn format_params() -> Vec<u8> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = libspa_sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = libspa_sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(pw::spa::pod::Object {
            type_: libspa_sys::SPA_TYPE_OBJECT_Format,
            id: libspa_sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .expect("serialising audio format")
    .0
    .into_inner()
}

fn delay_ns(time: &pw::stream::Time) -> i64 {
    let rate = time.rate();
    if rate.denom == 0 {
        return 0;
    }
    time.delay() * 1_000_000_000 * rate.num as i64 / rate.denom as i64
}

/// Runs `body` on a new thread with its own PipeWire main loop. The body sets up its stream, calls
/// `ready` and then runs the loop; this returns once `ready` was called (or the setup failed).
fn spawn_loop<F>(name: &str, body: F) -> Result<PwHandle>
where
    F: FnOnce(&pw::main_loop::MainLoopRc, &pw::core::CoreRc, &mut dyn FnMut()) -> Result<()>
        + Send
        + 'static,
{
    let (ready_tx, ready_rx) = mpsc::channel::<Result<pw::channel::Sender<()>>>();
    let thread = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            pw::init();
            let result = (|| -> Result<()> {
                let mainloop =
                    pw::main_loop::MainLoopRc::new(None).context("PipeWire main loop")?;
                let context =
                    pw::context::ContextRc::new(&mainloop, None).context("PipeWire context")?;
                let core = context.connect_rc(None).context("connecting to PipeWire")?;
                let (tx, rx) = pw::channel::channel::<()>();
                let _quit = rx.attach(mainloop.loop_(), {
                    let ml = mainloop.clone();
                    move |_| ml.quit()
                });
                let mut tx = Some(tx);
                let mut ready = || {
                    if let Some(tx) = tx.take() {
                        let _ = ready_tx.send(Ok(tx));
                    }
                };
                body(&mainloop, &core, &mut ready)
            })();
            if let Err(e) = result {
                // Only reaches the caller if `ready` wasn't called yet.
                let _ = ready_tx.send(Err(e));
            }
        })?;
    let sender = ready_rx
        .recv()
        .map_err(|_| anyhow!("PipeWire thread exited"))??;
    Ok(PwHandle {
        sender: Some(sender),
        thread: Some(thread),
    })
}

/// Captures the monitor of sink `target` into `capture`.
pub fn spawn_capture(target: &str, capture: CaptureProcessor) -> Result<PwHandle> {
    let target = target.to_string();
    spawn_loop("pw-capture", move |mainloop, core, ready| {
        let props = properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "hue-jack-capture",
            *pw::keys::NODE_DESCRIPTION => "hue-jack capture",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
            *pw::keys::TARGET_OBJECT => target.as_str(),
            *pw::keys::NODE_DONT_RECONNECT => "true",
            "node.dont-fallback" => "true",
            *pw::keys::NODE_LATENCY => LATENCY,
        };
        let stream = pw::stream::StreamBox::new(core, "hue-jack-capture", props)?;
        struct State {
            capture: CaptureProcessor,
            scratch: Vec<f32>,
        }
        let state = State {
            capture,
            scratch: vec![0.0; 32768],
        };
        let listener = stream
            .add_local_listener_with_user_data(state)
            .process(|stream, state| {
                // The samples in a sink monitor's buffer are what passed through the sink in this
                // cycle, so they were "captured" now. (The monitor's reported delay is the latency
                // upstream of the sink, which affects sound and light equally and is ignored.)
                let now = stream
                    .time()
                    .map(|t| t.now())
                    .unwrap_or_else(|_| super::monotonic_ns());
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let data = &mut buffer.datas_mut()[0];
                let (offset, size) = (data.chunk().offset() as usize, data.chunk().size() as usize);
                let Some(bytes) = data.data() else { return };
                let end = (offset + size).min(bytes.len());
                let bytes = &bytes[offset.min(end)..end];
                // SAFETY: f32 has no invalid bit patterns; alignment is checked by align_to.
                let (pre, samples, post) = unsafe { bytes.align_to::<f32>() };
                if pre.is_empty() && post.is_empty() {
                    state.capture.process(samples, now);
                } else {
                    let n = (bytes.len() / 4).min(state.scratch.len());
                    for (dst, b) in state.scratch[..n].iter_mut().zip(bytes.as_chunks::<4>().0) {
                        *dst = f32::from_le_bytes(*b);
                    }
                    let State { capture, scratch } = state;
                    capture.process(&scratch[..n - n % CHANNELS], now);
                }
            })
            .state_changed(|_, _, old, new| tracing::info!("capture stream: {old:?} → {new:?}"))
            .register()?;
        let params = format_params();
        let mut params = [Pod::from_bytes(&params).unwrap()];
        stream.connect(
            spa::utils::Direction::Input,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;
        ready();
        mainloop.run();
        drop(listener);
        Ok(())
    })
}

/// Plays the delay line's output to node `target`.
pub fn spawn_playback(target: &str, reader: DelayReader) -> Result<PwHandle> {
    let target = target.to_string();
    spawn_loop("pw-playback", move |mainloop, core, ready| {
        let props = properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Playback",
            *pw::keys::MEDIA_ROLE => "Music",
            *pw::keys::NODE_NAME => "hue-jack-playback",
            *pw::keys::NODE_DESCRIPTION => "hue-jack output",
            *pw::keys::TARGET_OBJECT => target.as_str(),
            *pw::keys::NODE_DONT_RECONNECT => "true",
            "node.dont-fallback" => "true",
            *pw::keys::NODE_LATENCY => LATENCY,
        };
        let stream = pw::stream::StreamBox::new(core, "hue-jack-playback", props)?;
        let listener = stream
            .add_local_listener_with_user_data(reader)
            .process(|stream, reader| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let requested = buffer.requested() as usize;
                // This buffer is heard after the device delay and after any buffers already queued.
                let play_ns = stream
                    .time()
                    .map(|t| {
                        let quantum = if requested > 0 { requested } else { 256 } as i64;
                        t.now()
                            + delay_ns(&t)
                            + super::frames_to_ns(t.queued_buffers() as i64 * quantum)
                    })
                    .unwrap_or_else(|_| super::monotonic_ns());
                let data = &mut buffer.datas_mut()[0];
                let stride = CHANNELS * 4;
                let mut frames = 0;
                if let Some(bytes) = data.data() {
                    let max = bytes.len() / stride;
                    frames = if requested > 0 {
                        requested.min(max)
                    } else {
                        max.min(256)
                    };
                    // SAFETY: f32 has no invalid bit patterns; alignment is checked by align_to_mut.
                    let (pre, samples, _) = unsafe { bytes.align_to_mut::<f32>() };
                    if pre.is_empty() {
                        reader.read(&mut samples[..frames * CHANNELS], play_ns);
                    } else {
                        frames = 0;
                    }
                }
                let chunk = data.chunk_mut();
                *chunk.offset_mut() = 0;
                *chunk.stride_mut() = stride as i32;
                *chunk.size_mut() = (frames * stride) as u32;
            })
            .state_changed(|_, _, old, new| tracing::info!("playback stream: {old:?} → {new:?}"))
            .register()?;
        let params = format_params();
        let mut params = [Pod::from_bytes(&params).unwrap()];
        stream.connect(
            spa::utils::Direction::Output,
            None,
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::RT_PROCESS,
            &mut params,
        )?;
        ready();
        mainloop.run();
        drop(listener);
        Ok(())
    })
}

/// A node from the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeInfo {
    pub name: String,
    pub media_class: String,
}

/// Lists nodes with a media class (one registry round trip).
pub fn list_nodes() -> Result<Vec<NodeInfo>> {
    use std::cell::RefCell;
    use std::rc::Rc;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pw-registry".into())
        .spawn(move || {
            let run = || -> Result<Vec<NodeInfo>> {
                pw::init();
                let mainloop = pw::main_loop::MainLoopRc::new(None)?;
                let context = pw::context::ContextRc::new(&mainloop, None)?;
                let core = context.connect_rc(None)?;
                let registry = core.get_registry_rc()?;
                let nodes = Rc::new(RefCell::new(Vec::new()));
                let _reg = registry
                    .add_listener_local()
                    .global({
                        let nodes = nodes.clone();
                        move |g| {
                            if g.type_ != pw::types::ObjectType::Node {
                                return;
                            }
                            if let Some(props) = g.props
                                && let (Some(name), Some(class)) =
                                    (props.get("node.name"), props.get("media.class"))
                            {
                                nodes.borrow_mut().push(NodeInfo {
                                    name: name.into(),
                                    media_class: class.into(),
                                });
                            }
                        }
                    })
                    .register();
                let pending = core.sync(0)?;
                let _core = core
                    .add_listener_local()
                    .done({
                        let ml = mainloop.clone();
                        move |id, seq| {
                            if id == pw::core::PW_ID_CORE && seq == pending {
                                ml.quit();
                            }
                        }
                    })
                    .register();
                mainloop.run();
                Ok(nodes.take())
            };
            let _ = tx.send(run());
        })?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| anyhow!("PipeWire registry timed out"))?
}

/// Resolves `output` (`auto` = first `alsa_output.*` sink) and refuses the input sink (feedback loop).
pub fn resolve_output(output: &str, input_sink: &str) -> Result<String> {
    let name = if output == "auto" {
        let nodes = list_nodes()?;
        pick_auto_output(&nodes)
            .ok_or_else(|| anyhow!("no alsa_output.* sink found for output = auto"))?
    } else {
        output.to_string()
    };
    check_not_input(&name, input_sink)?;
    Ok(name)
}

pub fn pick_auto_output(nodes: &[NodeInfo]) -> Option<String> {
    nodes
        .iter()
        .find(|n| n.media_class == "Audio/Sink" && n.name.starts_with("alsa_output."))
        .map(|n| n.name.clone())
}

pub fn check_not_input(output: &str, input_sink: &str) -> Result<()> {
    if output == input_sink {
        bail!("output {output:?} is the input sink: that would be a feedback loop");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_output_and_feedback_guard() {
        let nodes = vec![
            NodeInfo {
                name: "hue-jack-in".into(),
                media_class: "Audio/Sink".into(),
            },
            NodeInfo {
                name: "alsa_input.pci".into(),
                media_class: "Audio/Source".into(),
            },
            NodeInfo {
                name: "alsa_output.pci-0000_00_1f.3.analog-stereo".into(),
                media_class: "Audio/Sink".into(),
            },
        ];
        assert_eq!(
            pick_auto_output(&nodes).as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
        assert!(check_not_input("hue-jack-in", "hue-jack-in").is_err());
        assert!(check_not_input("alsa_output.x", "hue-jack-in").is_ok());
    }
}
