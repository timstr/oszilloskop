use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use eframe::egui;

fn main() {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Oszilloskop",
        native_options,
        Box::new(|cc| Box::new(OszilloskopApp::new(cc))),
    )
    .unwrap();
}

const BUFFER_SIZE: usize = 1024;

#[derive(Clone, Copy)]
struct AudioBuffer {
    l: [f32; BUFFER_SIZE],
    r: [f32; BUFFER_SIZE],
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self {
            l: [0.0; BUFFER_SIZE],
            r: [0.0; BUFFER_SIZE],
        }
    }
}

fn draw_line(
    mut x0: f32,
    mut y0: f32,
    mut x1: f32,
    mut y1: f32,
    image: &mut egui::ColorImage,
    exposure: f32,
) {
    // Xiaolin Wu's line algorithm
    // https://en.wikipedia.org/wiki/Xiaolin_Wu%27s_line_algorithm

    let intensity = (exposure.max(0.0) / (x1 - x0).hypot(y1 - y0).max(1.0)).min(1.0);

    let mut plot = |x: isize, y: isize, c: f32| {
        if x < 0 || x >= image.width() as isize || y < 0 || y >= image.height() as isize {
            return;
        }
        let c = (c * intensity).clamp(0.0, 1.0);
        let idx = (y as usize * image.width()) + x as usize;
        let [r, g, b, _] = image.pixels[idx].to_array();
        image.pixels[idx] = egui::Color32::from_rgba_premultiplied(
            r.saturating_add((c * 16.0).round() as u8),
            g.saturating_add((c * 255.0).round() as u8),
            b.saturating_add((c * 32.0).round() as u8),
            255,
        );
    };

    let steep = (y1 - y0).abs() > (x1 - x0).abs();

    if steep {
        std::mem::swap(&mut x0, &mut y0);
        std::mem::swap(&mut x1, &mut y1);
    }
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
        std::mem::swap(&mut y0, &mut y1);
    }

    let dx = x1 - x0;
    let dy = y1 - y0;

    let gradient = if dx.abs() < 1e-6 { 1.0 } else { dy / dx };

    // handle first endpoint
    let xend = x0.round();
    let yend = y0 + gradient * (xend - x0);
    let xgap = 1.0 - (x1 + 0.5).fract();
    let xpxl1 = xend as isize;
    let ypxl1 = yend.floor() as isize;

    if steep {
        plot(ypxl1, xpxl1, (1.0 - yend.fract()) * xgap);
        plot(ypxl1 + 1, xpxl1, yend.fract() * xgap);
    } else {
        plot(xpxl1, ypxl1, (1.0 - yend.fract()) * xgap);
        plot(xpxl1, ypxl1 + 1, yend.fract() * xgap);
    }
    let mut intery = yend + gradient;

    // handle second endpoint
    let xend = x1.round();
    let yend = y1 + gradient * (xend - x1);
    let xgap = (x1 + 0.5).fract();
    let xpxl2 = xend as isize;
    let ypxl2 = yend.floor() as isize;

    if steep {
        plot(ypxl2, xpxl2, (1.0 - yend.fract()) * xgap);
        plot(ypxl2 + 1, xpxl2, yend.fract() * xgap);
    } else {
        plot(xpxl2, ypxl2, (1.0 - yend.fract()) * xgap);
        plot(xpxl2, ypxl2 + 1, yend.fract() * xgap);
    }

    // main loop
    if steep {
        for x in (xpxl1 + 1)..(xpxl2) {
            plot(intery.floor() as isize, x, 1.0 - intery.fract());
            plot(intery.floor() as isize + 1, x, intery.fract());
            intery += gradient;
        }
    } else {
        for x in (xpxl1 + 1)..xpxl2 {
            plot(x, intery.floor() as isize, 1.0 - intery.fract());
            plot(x, intery.floor() as isize, intery.fract());
            intery += gradient;
        }
    }
}

struct OszilloskopApp {
    input_stream: cpal::Stream,
    buffer_receiver: spmcq::Reader<AudioBuffer>,
    exposure: f32,
    gain: f32,
    decay: f32,
    logarithmic_enable: bool,
    logarithmic_range: f32,
    rotation: u8,
    flip: bool,
    prev_sample: (f32, f32),
    image: egui::ColorImage,
    texture: Option<egui::TextureHandle>,
}

impl OszilloskopApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .expect("No input device available");

        let mut supported_configs_range = device
            .supported_input_configs()
            .expect("error while querying input configs");
        let supported_config = supported_configs_range
            .next()
            .expect("No supported input config:?")
            .with_sample_rate(cpal::SampleRate(48_000));
        let mut config: cpal::StreamConfig = supported_config.into();
        config.buffer_size = cpal::BufferSize::Fixed(BUFFER_SIZE as u32);

        config.channels = 2;

        let mut current_chunk = AudioBuffer::default();
        let mut chunk_cursor: usize = 0;

        let (rx, mut tx) = spmcq::ring_buffer::<AudioBuffer>(8);

        let data_callback = move |data: &[f32], _: &cpal::InputCallbackInfo| {
            for sample in data.chunks_exact(2) {
                current_chunk.l[chunk_cursor] = sample[0];
                current_chunk.r[chunk_cursor] = sample[1];
                chunk_cursor += 1;
                if chunk_cursor == BUFFER_SIZE {
                    chunk_cursor = 0;
                    tx.write(current_chunk);
                }
            }
        };

        let stream = device
            .build_input_stream(
                &config,
                data_callback,
                |err| {
                    panic!("CPAL Input stream encountered an error: {}", err);
                },
                None,
            )
            .unwrap();
        stream.play().unwrap();

        OszilloskopApp {
            input_stream: stream,
            buffer_receiver: rx,
            exposure: 5.0,
            gain: 0.7,
            logarithmic_enable: false,
            logarithmic_range: 15.0,
            decay: 0.3,
            rotation: 1,
            flip: true,
            prev_sample: (0.0, 0.0),
            image: egui::ColorImage::new([512, 512], egui::Color32::BLACK),
            texture: None,
        }
    }

    fn update_image(&mut self) {
        while let Some(buffer) = self.buffer_receiver.read().value() {
            let img = &mut self.image;
            let w = img.width() as f32;
            let h = img.height() as f32;

            for c in &mut img.pixels {
                let [r, g, b, _] = c.to_array();
                *c = egui::Color32::from_rgba_premultiplied(
                    r.saturating_sub(((r as f32 * self.decay).round() as u8).max(1)),
                    g.saturating_sub(((g as f32 * self.decay).round() as u8).max(1)),
                    b.saturating_sub(((b as f32 * self.decay).round() as u8).max(1)),
                    255,
                );
            }

            let theta = -(self.rotation as f32) * std::f32::consts::FRAC_PI_4;

            let (sin_theta, cos_theta) = theta.sin_cos();

            let min_val = (-self.logarithmic_range).exp2();
            let inv_log_scale: f32 = -1.0 / self.logarithmic_range;

            let mut s_prev = self.prev_sample;
            for s in buffer.l.iter().cloned().zip(buffer.r.iter().cloned()) {
                let s = if self.flip { (s.1, s.0) } else { s };
                let s = (
                    s.0 * cos_theta + s.1 * sin_theta,
                    s.0 * -sin_theta + s.1 * cos_theta,
                );

                let s = if self.logarithmic_enable {
                    let length = s.0.hypot(s.1);
                    let log_len = length.max(min_val).log2();
                    let t = (log_len + self.logarithmic_range) * inv_log_scale;
                    let k = t / length;
                    (k * s.0, k * s.1)
                } else {
                    s
                };

                let x0 = (0.5 + 0.5 * self.gain * s_prev.0).clamp(0.0, 1.0) * w;
                let y0 = (0.5 - 0.5 * self.gain * s_prev.1).clamp(0.0, 1.0) * h;
                let x1 = (0.5 + 0.5 * self.gain * s.0).clamp(0.0, 1.0) * w;
                let y1 = (0.5 - 0.5 * self.gain * s.1).clamp(0.0, 1.0) * h;

                draw_line(x0, y0, x1, y1, img, self.exposure);
                s_prev = s;
            }
            self.prev_sample = s_prev;
        }
    }
}

impl eframe::App for OszilloskopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical(|ui| {
                self.update_image();

                let texture_id = match self.texture.as_mut() {
                    Some(texture) => {
                        texture.set(self.image.clone(), egui::TextureOptions::default());
                        texture.id()
                    }
                    None => {
                        let texture = ui.ctx().load_texture(
                            "oscilloscope",
                            self.image.clone(),
                            egui::TextureOptions::default(),
                        );
                        let id = texture.id();
                        self.texture = Some(texture);
                        id
                    }
                };

                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.exposure, 0.0..=100.0).logarithmic(true));
                    ui.separator();
                    ui.label("Beam Strength");
                });

                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.gain, 0.0..=100.0).logarithmic(true));
                    ui.separator();
                    ui.label("Gain");
                });

                ui.horizontal(|ui| {
                    ui.toggle_value(&mut self.logarithmic_enable, "Logarithmic");
                    ui.separator();
                    if self.logarithmic_enable {
                        ui.add(egui::Slider::new(&mut self.logarithmic_range, 0.0..=30.0));
                        ui.separator();
                        ui.label("Range");
                    }
                });

                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.decay, 0.0..=1.0).logarithmic(true));
                    ui.separator();
                    ui.label("Decay");
                });

                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut self.rotation, 0..=8));
                    ui.separator();
                    ui.label("Rotation");
                });

                ui.horizontal(|ui| {
                    ui.add(egui::Checkbox::new(&mut self.flip, ""));
                    ui.separator();
                    ui.label("Flip");
                });

                let available_space = ui.available_size().min_elem();

                let rect = ui.allocate_space(egui::Vec2::splat(available_space)).1;

                let painter = ui.painter();

                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

                let tint = egui::Color32::WHITE;

                painter.image(texture_id, rect, uv, tint);

                ui.ctx().request_repaint();
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.input_stream.pause().unwrap();
    }
}
