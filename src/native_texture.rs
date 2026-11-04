//! Deterministic native fabric patterns. No browser canvas or generated web textures.
use crate::ara::rgb;
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use serde_json::Value;
pub fn fabric_image(material: &Value) -> Image {
    let pattern = material["pattern"].as_str().unwrap_or("solid");
    let primary = rgb(material["color"].as_str().unwrap_or("#6b7984"));
    let secondary = rgb(material["color2"].as_str().unwrap_or("#20262f"));
    let size = 256u32;
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let seed = pattern
        .bytes()
        .fold(2166136261u32, |s, b| s.wrapping_mul(16777619) ^ b as u32);
    for y in 0..size {
        for x in 0..size {
            let u = x as f32 / size as f32;
            let v = y as f32 / size as f32;
            let noise = |x: u32, y: u32| {
                let mut h = seed ^ (x.wrapping_mul(374761393)) ^ (y.wrapping_mul(668265263));
                h = (h ^ (h >> 13)).wrapping_mul(1274126177);
                ((h ^ (h >> 16)) & 65535) as f32 / 65535.
            };
            let weave = if (x + y) % 2 == 0 { 0.96 } else { 1.04 };
            let a = match pattern {
                "solid" => 0.,
                "pinstripe" | "suiting" | "stripe" | "stripes" => {
                    if x % 32 < 2 {
                        0.65
                    } else {
                        0.
                    }
                }
                "check" | "checks" | "plaid" => {
                    if (x / 24 + y / 24) % 2 == 0 {
                        0.7
                    } else {
                        0.05
                    }
                }
                "camo" | "camouflage" => {
                    let n = noise(x / 24, y / 24);
                    if n > 0.5 { 0.85 } else { n * 0.6 }
                }
                "leather" => noise(x / 3, y / 3) * 0.22,
                "denim" => {
                    if (x + y) % 7 < 2 {
                        0.35
                    } else {
                        0.05
                    }
                }
                "kente" | "aso_oke" | "aso-oke" => {
                    let check = (x / 16 + y / 40) % 3;
                    if check == 0 {
                        0.95
                    } else if (x + y) % 24 < 7 {
                        0.5
                    } else {
                        0.03
                    }
                }
                "ankara" => {
                    if ((u * 8.).fract() - 0.5).powi(2) + ((v * 8.).fract() - 0.5).powi(2) < 0.09 {
                        0.9
                    } else {
                        0.05
                    }
                }
                "adire" => {
                    let w = ((u * 40.).sin() * (v * 34.).cos()).abs();
                    if w > 0.75 { 0.85 } else { 0.05 }
                }
                "khaki" | "linen" | "cotton" => noise(x, y) * 0.08,
                "tattered" => {
                    if noise(x / 16, y / 8) > 0.83 {
                        0.85
                    } else {
                        noise(x, y) * 0.1
                    }
                }
                _ => {
                    let rate = 3. + (seed % 13) as f32;
                    (((u * rate * std::f32::consts::TAU).sin()
                        * (v * (rate + 2.) * std::f32::consts::TAU).cos())
                        * 0.5
                        + 0.5)
                        * 0.65
                }
            };
            for k in 0..3 {
                data.push(
                    ((primary[k] * (1. - a) + secondary[k] * a) * 255. * weave).clamp(0., 255.)
                        as u8,
                );
            }
            data.push(255);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    image
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn patterns_and_colors_change_pixels_deterministically() {
        let a = json!({"pattern":"camo","color":"#4a6545","color2":"#192e1a"});
        let b = json!({"pattern":"kente","color":"#4a6545","color2":"#192e1a"});
        assert_eq!(fabric_image(&a).data, fabric_image(&a).data);
        assert_ne!(fabric_image(&a).data, fabric_image(&b).data);
    }
}

/// Recolors the imported two-tone pattern mask, preserving its authored design.
/// Only patterns without a prepared mask use the procedural fallback above.
pub fn recolor_pattern(root: &std::path::Path, material: &Value) -> Option<Image> {
    let pattern = material["pattern"].as_str()?;
    let image = image::open(
        root.join("textures")
            .join(format!("wardrobe_pattern_{pattern}.png")),
    )
    .ok()?
    .into_rgba8();
    let (width, height) = image.dimensions();
    let mut data = image.into_raw();
    let a = rgb(material["color"].as_str().unwrap_or("#ffffff"));
    let b = rgb(material["color2"].as_str().unwrap_or("#606060"));
    for pixel in data.chunks_exact_mut(4) {
        let luminance =
            (pixel[0] as f32 * 0.2126 + pixel[1] as f32 * 0.7152 + pixel[2] as f32 * 0.0722) / 255.;
        let blend = ((1. - luminance) / (1. - 96. / 255.)).clamp(0., 1.);
        for k in 0..3 {
            pixel[k] = ((a[k] * (1. - blend) + b[k] * blend) * 255.)
                .round()
                .clamp(0., 255.) as u8;
        }
    }
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    Some(image)
}

#[cfg(test)]
mod mask_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn authored_patterns_keep_their_designs_and_accept_both_colors() {
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/native"));
        let a = json!({"pattern":"ankara_floral","color":"#cc3300","color2":"#1177bb"});
        let image = recolor_pattern(root, &a).expect("authored floral pattern missing");
        assert_ne!(image.data, fabric_image(&a).data);
        let mut b = a.clone();
        b["color2"] = json!("#223344");
        assert_ne!(image.data, recolor_pattern(root, &b).unwrap().data);
    }
}

/// Suppress photographic pores/weave while retaining authored color regions.
/// Used by preview and export; alpha coverage is deliberately unchanged.
pub fn soften_diffuse(image: &mut image::RgbaImage, strength: f32) {
    let strength = strength.clamp(0., 1.);
    if strength == 0. || !strength.is_finite() {
        return;
    }
    let sigma = (image.width().max(image.height()) as f32 / 256.).clamp(0.5, 12.);
    let broad = image::imageops::blur(image, sigma);
    for (pixel, blurred) in image.pixels_mut().zip(broad.pixels()) {
        for channel in 0..3 {
            pixel[channel] = (pixel[channel] as f32 * (1. - strength * 0.85)
                + blurred[channel] as f32 * strength * 0.85)
                .round() as u8;
        }
    }
}

/// Cartoon-mode texture pass, chosen by the material's `cartoonKind` (set in
/// `native_cartoon::soften_material`) and scaled by its `cartoonDetail`.
pub fn cartoon_texture(image: &mut image::RgbaImage, material: &Value) {
    let strength = material["cartoonDetail"].as_f64().unwrap_or(0.) as f32;
    let strength = if strength.is_finite() { strength.clamp(0., 1.) } else { 0. };
    if strength == 0. {
        return;
    }
    match material["cartoonKind"].as_str() {
        Some("skin") => cartoon_skin(image, strength),
        Some("eyes") => cartoon_eyes(image, strength),
        Some("brows") => solid_strands(image, strength, 1. / 64.),
        Some("lashes") => solid_strands(image, strength, 1. / 60.),
        // Painted stubble: the dots merge into one soft beard shade.
        Some("painted") => {
            solid_strands(image, strength, 1. / 120.);
            let soft = broad_blur(image, image.width() as f32 / 200.);
            for (p, b) in image.pixels_mut().zip(soft.pixels()) {
                p[3] = (b[3] as f32 * (1. - 0.45 * strength)).round() as u8;
            }
        }
        _ => soften_diffuse(image, strength),
    }
}

/// Gaussian blur done at reduced resolution: wide radii stay cheap on 2K skins.
fn broad_blur(image: &image::RgbaImage, sigma: f32) -> image::RgbaImage {
    use image::imageops::{FilterType, blur, resize};
    let (w, h) = image.dimensions();
    let step = (sigma / 2.).floor().clamp(1., 16.) as u32;
    if step == 1 {
        return blur(image, sigma);
    }
    let small = resize(image, (w / step).max(1), (h / step).max(1), FilterType::Triangle);
    resize(&blur(&small, sigma / step as f32), w, h, FilterType::Triangle)
}

/// Flat, toy-like skin: keep only broad regional colour (lips, cheeks, nipples,
/// nails), lift the tone a little and drop the photographic contrast.
fn cartoon_skin(image: &mut image::RgbaImage, strength: f32) {
    let sigma = image.width().max(image.height()) as f32 / 110.;
    let broad = broad_blur(image, sigma);
    let lift = 1. - 0.16 * strength;
    for (pixel, blurred) in image.pixels_mut().zip(broad.pixels()) {
        for c in 0..3 {
            let flat = (blurred[c] as f32 / 255.).powf(lift) * 255.;
            pixel[c] = (pixel[c] as f32 + (flat - pixel[c] as f32) * strength).round() as u8;
        }
    }
}

/// MakeHuman eye atlas: two eyeballs, irises at these UV centres (image space).
const IRIS_CENTRES: [[f32; 2]; 2] = [[0.289, 0.707], [0.705, 0.295]];
const IRIS_RADIUS: f32 = 0.11;
/// Cartoon iris radius (UV units) and pupil share of it.
const CARTOON_IRIS: f32 = 0.125;
const CARTOON_PUPIL: f32 = 0.56;

/// Redraws the realistic eyes as cartoon eyes: flat white sclera, a large dark
/// iris keeping the original hue, a big pupil and a painted catch-light.
fn cartoon_eyes(image: &mut image::RgbaImage, strength: f32) {
    let (w, h) = image.dimensions();
    let size = w.min(h) as f32;
    let mut iris = [0f32; 3];
    let mut count = 0f32;
    for (x, y, p) in image.enumerate_pixels() {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let d = IRIS_CENTRES
            .iter()
            .map(|c| ((u - c[0]).powi(2) + (v - c[1]).powi(2)).sqrt())
            .fold(f32::INFINITY, f32::min);
        if d > IRIS_RADIUS * 0.5 && d < IRIS_RADIUS * 0.85 {
            for c in 0..3 {
                iris[c] += p[c] as f32;
            }
            count += 1.;
        }
    }
    let iris = iris.map(|c| c / count.max(1.));
    let aa = 1.5 / size;
    let edge = |d: f32, r: f32| ((r - d) / aa + 0.5).clamp(0., 1.);
    for (x, y, p) in image.enumerate_pixels_mut() {
        let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
        let (centre, d) = IRIS_CENTRES
            .iter()
            .map(|c| (c, ((u - c[0]).powi(2) + (v - c[1]).powi(2)).sqrt()))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let t = (d / CARTOON_IRIS).min(1.);
        let mut colour = [242., 240., 236.];
        // Iris: original hue, darkened, lighter towards the rim, with a dark limbal ring.
        let shade = 0.45 + 0.35 * t * t - 0.35 * ((t - 0.9) / 0.1).clamp(0., 1.);
        let disc = edge(d, CARTOON_IRIS);
        for c in 0..3 {
            colour[c] += (iris[c] * shade - colour[c]) * disc;
        }
        let pupil = edge(d, CARTOON_IRIS * CARTOON_PUPIL);
        for c in 0..3 {
            colour[c] += (14. - colour[c]) * pupil;
        }
        let (hu, hv) = (
            centre[0] - CARTOON_IRIS * 0.35,
            centre[1] - CARTOON_IRIS * 0.35,
        );
        let glint = edge(((u - hu).powi(2) + (v - hv).powi(2)).sqrt(), CARTOON_IRIS * 0.2);
        for c in 0..3 {
            colour[c] += (255. - colour[c]) * glint;
            p[c] = (p[c] as f32 + (colour[c] - p[c] as f32) * strength).round() as u8;
        }
    }
}

/// Hair-strand cards (brows, lashes) become solid painted shapes: the alpha is
/// widened and hardened, the strands' colour flattened to its mean.
fn solid_strands(image: &mut image::RgbaImage, strength: f32, spread: f32) {
    let sigma = image.width().max(image.height()) as f32 * spread;
    let broad = broad_blur(image, sigma);
    let (mut sum, mut weight) = ([0f32; 3], 0f32);
    for p in image.pixels() {
        let a = p[3] as f32 / 255.;
        for c in 0..3 {
            sum[c] += p[c] as f32 * a;
        }
        weight += a;
    }
    let mean = sum.map(|c| c / weight.max(1.));
    for (p, b) in image.pixels_mut().zip(broad.pixels()) {
        let a = b[3] as f32 / 255.;
        let solid = ((a - 0.06) / 0.22).clamp(0., 1.);
        let solid = solid * solid * (3. - 2. * solid) * 255.;
        p[3] = (p[3] as f32 + (solid - p[3] as f32) * strength).round() as u8;
        for c in 0..3 {
            p[c] = (p[c] as f32 + (mean[c] - p[c] as f32) * strength).round() as u8;
        }
    }
}

#[cfg(test)]
mod cartoon_texture_tests {
    use super::*;
    #[test]
    fn photographic_detail_is_reduced_without_changing_alpha_or_disabled_pixels() {
        let mut texture = image::RgbaImage::from_fn(16, 16, |x, y| {
            image::Rgba([
                if (x + y) % 2 == 0 { 40 } else { 200 },
                80,
                120,
                (x * 16) as u8,
            ])
        });
        let original = texture.clone();
        soften_diffuse(&mut texture, 0.);
        assert_eq!(texture, original);
        soften_diffuse(&mut texture, 1.);
        for (pixel, before) in texture.pixels().zip(original.pixels()) {
            assert_eq!(pixel[3], before[3]);
            assert_eq!(pixel[1], 80);
            assert!(pixel[0] > 40 && pixel[0] < 200);
        }
    }

    #[test]
    fn cartoon_eyes_paint_a_large_dark_iris() {
        let mut eye = image::RgbaImage::from_pixel(256, 256, image::Rgba([170, 165, 160, 255]));
        let material = serde_json::json!({"cartoonDetail": 1.0, "cartoonKind": "eyes"});
        cartoon_texture(&mut eye, &material);
        // Inside the cartoon iris but outside the realistic one: now dark.
        let (x, y) = (((0.289 + 0.118) * 256.) as u32, (0.707 * 256.) as u32);
        assert!(eye.get_pixel(x, y)[0] < 120);
        // Far from both irises: flat white sclera.
        assert!(eye.get_pixel(2, 2)[0] > 230);
    }

    #[test]
    fn cartoon_strands_become_solid() {
        let mut brow = image::RgbaImage::from_fn(256, 256, |x, y| {
            let on = (80..176).contains(&y) && x % 3 == 0;
            image::Rgba([20, 20, 20, if on { 255 } else { 0 }])
        });
        let material = serde_json::json!({"cartoonDetail": 1.0, "cartoonKind": "brows"});
        cartoon_texture(&mut brow, &material);
        assert!(brow.get_pixel(126, 128)[3] > 200 && brow.get_pixel(127, 128)[3] > 200);
        assert_eq!(brow.get_pixel(128, 8)[3], 0);
    }
}
