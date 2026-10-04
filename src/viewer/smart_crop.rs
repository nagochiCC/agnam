//! Conservative detection of low-information borders in decoded RGBA images.
//!
//! The constants intentionally live together: real manga pages will guide later
//! tuning without coupling the detector to GTK rendering.

const CHANNELS: usize = 4;
const MAX_SAMPLE_DIMENSION: usize = 640;
const MIN_CONSECUTIVE_CONTENT_LINES: usize = 3;
const MIN_DARK_OCCUPANCY_NUMERATOR: usize = 3;
const MIN_DARK_OCCUPANCY_DENOMINATOR: usize = 100;
const MAX_EDGE_ARTIFACT_LINES: usize = 3;
const MIN_EDGE_BLANK_GAP_LINES: usize = 3;
const OUTER_ARTIFACT_BAND_NUMERATOR: usize = 5;
const OUTER_ARTIFACT_BAND_DENOMINATOR: usize = 100;
const DENSE_EDGE_ARTIFACT_NUMERATOR: usize = 1;
const DENSE_EDGE_ARTIFACT_DENOMINATOR: usize = 4;
const WIDE_ARTIFACT_SPAN_NUMERATOR: usize = 3;
const WIDE_ARTIFACT_SPAN_DENOMINATOR: usize = 4;
const MAX_WIDE_ARTIFACT_OCCUPANCY_NUMERATOR: usize = 1;
const MAX_WIDE_ARTIFACT_OCCUPANCY_DENOMINATOR: usize = 10;
const TRANSPARENT_BACKGROUND_NUMERATOR: usize = 1;
const TRANSPARENT_BACKGROUND_DENOMINATOR: usize = 10;
const DARK_BACKGROUND_LUMA_MAX: u8 = 64;
const LUMA_GRADIENT: u8 = 22;
const TRANSPARENT_ALPHA: u8 = 20;
const MIN_RETAINED_PIXELS: usize = 16;
const MIN_RETAINED_FRACTION_DENOMINATOR: usize = 4;
const SAFETY_PADDING_MIN: usize = 2;
const SAFETY_PADDING_MAX: usize = 16;
const SAFETY_PADDING_DIVISOR: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CropRect {
    pub(super) x: usize,
    pub(super) y: usize,
    pub(super) width: usize,
    pub(super) height: usize,
}

pub(super) enum DetectResult {
    Complete(Option<CropRect>),
    Cancelled,
}

impl CropRect {
    pub(super) fn full(width: usize, height: usize) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct RowInkProfile {
    has_content: bool,
    artifact_like: bool,
}

/// Returns a safe vertical crop rectangle only when content is consistently
/// detected away from at least one outer edge. Invalid or uncertain input
/// returns None.
///
/// Spread assets are evaluated as two logical page-width regions, but the final
/// crop rectangle remains common to the physical asset and always preserves its
/// full width and original center split.
pub(super) fn detect_rgba(
    pixels: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    spread: bool,
) -> Option<CropRect> {
    match detect_rgba_cancellable(pixels, stride, width, height, spread, || false) {
        DetectResult::Complete(crop) => crop,
        DetectResult::Cancelled => None,
    }
}

pub(super) fn detect_rgba_cancellable(
    pixels: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    spread: bool,
    mut cancelled: impl FnMut() -> bool,
) -> DetectResult {
    let mut was_cancelled = false;
    let detected = (|| {
        if width == 0
            || height == 0
            || stride < width.checked_mul(CHANNELS)?
            || pixels.len() < stride.checked_mul(height)?
        {
            return None;
        }

        let sample_step = width.max(height).div_ceil(MAX_SAMPLE_DIMENSION).max(1);
        let sample_height = height.div_ceil(sample_step);
        if sample_height < MIN_CONSECUTIVE_CONTENT_LINES {
            return None;
        }

        let split_regions = [(0, width / 2), (width / 2, width)];
        let single_region = [(0, width)];
        let regions: &[(usize, usize)] = if spread && width >= 2 {
            &split_regions
        } else {
            &single_region
        };

        let mut samples = Vec::with_capacity(width.div_ceil(sample_step));
        let mut row_profiles = Vec::with_capacity(sample_height);
        for sample_y in 0..sample_height {
            if sample_y % 16 == 0 && cancelled() {
                was_cancelled = true;
                return None;
            }
            row_profiles.push(combined_row_profile(
                pixels,
                stride,
                height,
                sample_y,
                sample_step,
                regions,
                &mut samples,
            ));
        }

        let top = first_content_with_edge_artifact_guard(&row_profiles)? * sample_step;
        let bottom = last_content_with_edge_artifact_guard(&row_profiles)? * sample_step;

        let padding = (width.max(height) / SAFETY_PADDING_DIVISOR)
            .clamp(SAFETY_PADDING_MIN, SAFETY_PADDING_MAX);
        let y = top.saturating_sub(padding);
        let bottom = (bottom + sample_step + padding).min(height);
        let crop = CropRect {
            x: 0,
            y,
            width,
            height: bottom.checked_sub(y)?,
        };

        if crop == CropRect::full(width, height)
            || crop.height < MIN_RETAINED_PIXELS.min(height)
            || crop.height * MIN_RETAINED_FRACTION_DENOMINATOR < height
        {
            return None;
        }
        Some(crop)
    })();
    if was_cancelled {
        DetectResult::Cancelled
    } else {
        DetectResult::Complete(detected)
    }
}

fn combined_row_profile(
    pixels: &[u8],
    stride: usize,
    height: usize,
    sample_y: usize,
    sample_step: usize,
    regions: &[(usize, usize)],
    samples: &mut Vec<[u8; CHANNELS]>,
) -> RowInkProfile {
    let y_start = (sample_y * sample_step).min(height - 1);
    let y_end = (y_start + sample_step).min(height);

    let mut has_content = false;
    let mut all_content_is_artifact_like = true;
    for &(x_start, x_end) in regions {
        let profile = region_row_profile(
            pixels,
            stride,
            x_start,
            x_end,
            y_start,
            y_end,
            sample_step,
            samples,
        );
        if profile.has_content {
            has_content = true;
            all_content_is_artifact_like &= profile.artifact_like;
        }
    }

    RowInkProfile {
        has_content,
        artifact_like: has_content && all_content_is_artifact_like,
    }
}

fn region_row_profile(
    pixels: &[u8],
    stride: usize,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    sample_step: usize,
    samples: &mut Vec<[u8; CHANNELS]>,
) -> RowInkProfile {
    if x_start >= x_end || y_start >= y_end {
        return RowInkProfile::default();
    }

    samples.clear();
    let mut x = x_start;
    while x < x_end {
        let block_end = (x + sample_step).min(x_end);
        samples.push(average_block_rgba(
            pixels, stride, x, block_end, y_start, y_end,
        ));
        x += sample_step;
    }
    row_ink_profile(samples)
}

fn average_block_rgba(
    pixels: &[u8],
    stride: usize,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
) -> [u8; CHANNELS] {
    let mut red = 0_u64;
    let mut green = 0_u64;
    let mut blue = 0_u64;
    let mut alpha = 0_u64;
    let mut count = 0_u64;
    for y in y_start..y_end {
        let row_start = y * stride + x_start * CHANNELS;
        let row_end = y * stride + x_end * CHANNELS;
        for pixel in pixels[row_start..row_end].chunks_exact(CHANNELS) {
            red += u64::from(pixel[0]);
            green += u64::from(pixel[1]);
            blue += u64::from(pixel[2]);
            alpha += u64::from(pixel[3]);
            count += 1;
        }
    }
    if count == 0 {
        return [0; CHANNELS];
    }
    [
        (red / count) as u8,
        (green / count) as u8,
        (blue / count) as u8,
        (alpha / count) as u8,
    ]
}

fn row_ink_profile(samples: &[[u8; CHANNELS]]) -> RowInkProfile {
    if samples.is_empty() {
        return RowInkProfile::default();
    }

    let transparent_count = samples
        .iter()
        .filter(|pixel| pixel[3] <= TRANSPARENT_ALPHA)
        .count();
    let transparent_background = transparent_count * TRANSPARENT_BACKGROUND_DENOMINATOR
        >= samples.len() * TRANSPARENT_BACKGROUND_NUMERATOR;

    let background = if transparent_background {
        None
    } else {
        opaque_luma_background(samples)
    };
    if background.is_some_and(|background| background <= DARK_BACKGROUND_LUMA_MAX) {
        return RowInkProfile {
            has_content: true,
            artifact_like: false,
        };
    }

    let outer_band = ((samples.len() * OUTER_ARTIFACT_BAND_NUMERATOR)
        .div_ceil(OUTER_ARTIFACT_BAND_DENOMINATOR))
    .max(1);
    let mut active_count = 0;
    let mut first = None;
    let mut last = 0;
    let mut confined_to_outer_bands = true;
    for (index, pixel) in samples.iter().enumerate() {
        let active = if transparent_background {
            pixel[3] > TRANSPARENT_ALPHA
        } else {
            background.is_some_and(|background| {
                pixel[3] > TRANSPARENT_ALPHA
                    && background.saturating_sub(luma(pixel)) >= LUMA_GRADIENT
            })
        };
        if active {
            active_count += 1;
            first.get_or_insert(index);
            last = index;
            confined_to_outer_bands &=
                index < outer_band || index >= samples.len().saturating_sub(outer_band);
        }
    }
    let has_content = active_count * MIN_DARK_OCCUPANCY_DENOMINATOR
        >= samples.len() * MIN_DARK_OCCUPANCY_NUMERATOR;
    if !has_content {
        return RowInkProfile::default();
    }

    let first = first.unwrap();
    let span = last - first + 1;
    let dense_edge_band = active_count * DENSE_EDGE_ARTIFACT_DENOMINATOR
        >= samples.len() * DENSE_EDGE_ARTIFACT_NUMERATOR;
    let sparse_and_wide = span * WIDE_ARTIFACT_SPAN_DENOMINATOR
        >= samples.len() * WIDE_ARTIFACT_SPAN_NUMERATOR
        && active_count * MAX_WIDE_ARTIFACT_OCCUPANCY_DENOMINATOR
            <= samples.len() * MAX_WIDE_ARTIFACT_OCCUPANCY_NUMERATOR;

    RowInkProfile {
        has_content,
        artifact_like: confined_to_outer_bands || dense_edge_band || sparse_and_wide,
    }
}

fn opaque_luma_background(samples: &[[u8; CHANNELS]]) -> Option<u8> {
    let mut histogram = [0_usize; 256];
    let mut opaque_count = 0;
    for pixel in samples.iter().filter(|pixel| pixel[3] > TRANSPARENT_ALPHA) {
        histogram[usize::from(luma(pixel))] += 1;
        opaque_count += 1;
    }
    if opaque_count == 0 {
        return None;
    }

    let target = opaque_count * 3 / 4;
    let mut seen = 0;
    histogram.iter().enumerate().find_map(|(value, count)| {
        seen += count;
        (seen > target).then_some(value as u8)
    })
}

fn luma(pixel: &[u8; CHANNELS]) -> u8 {
    ((u16::from(pixel[0]) * 54 + u16::from(pixel[1]) * 183 + u16::from(pixel[2]) * 19) / 256) as u8
}

fn first_content_with_edge_artifact_guard(rows: &[RowInkProfile]) -> Option<usize> {
    let first = first_persistent_content_from(rows, 0)?;
    if first != 0 {
        return Some(first);
    }

    let run_end = rows
        .iter()
        .position(|row| !row.has_content)
        .unwrap_or(rows.len());
    if !edge_run_can_be_ignored(&rows[..run_end]) {
        return Some(0);
    }

    let gap_end = run_end.checked_add(MIN_EDGE_BLANK_GAP_LINES)?;
    if gap_end > rows.len() || rows[run_end..gap_end].iter().any(|row| row.has_content) {
        return Some(0);
    }

    first_persistent_content_from(rows, gap_end).or(Some(0))
}

fn last_content_with_edge_artifact_guard(rows: &[RowInkProfile]) -> Option<usize> {
    let last = last_persistent_content_before(rows, rows.len())?;
    if last + 1 != rows.len() {
        return Some(last);
    }

    let run_start = rows
        .iter()
        .rposition(|row| !row.has_content)
        .map(|index| index + 1)
        .unwrap_or(0);
    if !edge_run_can_be_ignored(&rows[run_start..]) {
        return Some(rows.len() - 1);
    }

    if run_start < MIN_EDGE_BLANK_GAP_LINES {
        return Some(rows.len() - 1);
    }
    let gap_start = run_start - MIN_EDGE_BLANK_GAP_LINES;
    if rows[gap_start..run_start].iter().any(|row| row.has_content) {
        return Some(rows.len() - 1);
    }

    last_persistent_content_before(rows, gap_start).or(Some(rows.len() - 1))
}

fn edge_run_can_be_ignored(run: &[RowInkProfile]) -> bool {
    !run.is_empty()
        && run.len() <= MAX_EDGE_ARTIFACT_LINES
        && run.iter().all(|row| row.artifact_like)
}

fn first_persistent_content_from(rows: &[RowInkProfile], start: usize) -> Option<usize> {
    let last_start = rows.len().checked_sub(MIN_CONSECUTIVE_CONTENT_LINES)?;
    if start > last_start {
        return None;
    }
    (start..=last_start).find(|&candidate| {
        rows[candidate..candidate + MIN_CONSECUTIVE_CONTENT_LINES]
            .iter()
            .all(|row| row.has_content)
    })
}

fn last_persistent_content_before(rows: &[RowInkProfile], end_exclusive: usize) -> Option<usize> {
    let end_exclusive = end_exclusive.min(rows.len());
    let last_start = end_exclusive.checked_sub(MIN_CONSECUTIVE_CONTENT_LINES)?;
    (0..=last_start)
        .rev()
        .find(|&candidate| {
            rows[candidate..candidate + MIN_CONSECUTIVE_CONTENT_LINES]
                .iter()
                .all(|row| row.has_content)
        })
        .map(|start| start + MIN_CONSECUTIVE_CONTENT_LINES - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: usize, height: usize, fill: [u8; 4]) -> Vec<u8> {
        [fill]
            .repeat(width * height)
            .into_iter()
            .flatten()
            .collect()
    }

    fn set_pixel(pixels: &mut [u8], width: usize, x: usize, y: usize, pixel: [u8; 4]) {
        let offset = (y * width + x) * CHANNELS;
        pixels[offset..offset + CHANNELS].copy_from_slice(&pixel);
    }

    fn fill_rect(
        pixels: &mut [u8],
        width: usize,
        x: usize,
        y: usize,
        rect_width: usize,
        rect_height: usize,
        pixel: [u8; 4],
    ) {
        for row in y..y + rect_height {
            for column in x..x + rect_width {
                set_pixel(pixels, width, column, row, pixel);
            }
        }
    }

    fn reference_average_block_rgba(
        pixels: &[u8],
        stride: usize,
        x_start: usize,
        x_end: usize,
        y_start: usize,
        y_end: usize,
    ) -> [u8; CHANNELS] {
        let mut sums = [0_u64; CHANNELS];
        let mut count = 0_u64;
        for y in y_start..y_end {
            for x in x_start..x_end {
                let offset = y * stride + x * CHANNELS;
                for channel in 0..CHANNELS {
                    sums[channel] += u64::from(pixels[offset + channel]);
                }
                count += 1;
            }
        }
        if count == 0 {
            return [0; CHANNELS];
        }
        [
            (sums[0] / count) as u8,
            (sums[1] / count) as u8,
            (sums[2] / count) as u8,
            (sums[3] / count) as u8,
        ]
    }

    fn reference_row_ink_profile(samples: &[[u8; CHANNELS]]) -> RowInkProfile {
        if samples.is_empty() {
            return RowInkProfile::default();
        }
        let transparent_count = samples
            .iter()
            .filter(|pixel| pixel[3] <= TRANSPARENT_ALPHA)
            .count();
        let active_indices = if transparent_count * TRANSPARENT_BACKGROUND_DENOMINATOR
            >= samples.len() * TRANSPARENT_BACKGROUND_NUMERATOR
        {
            samples
                .iter()
                .enumerate()
                .filter_map(|(index, pixel)| (pixel[3] > TRANSPARENT_ALPHA).then_some(index))
                .collect::<Vec<_>>()
        } else {
            let mut opaque_lumas = samples
                .iter()
                .filter(|pixel| pixel[3] > TRANSPARENT_ALPHA)
                .map(luma)
                .collect::<Vec<_>>();
            if opaque_lumas.is_empty() {
                return RowInkProfile::default();
            }
            opaque_lumas.sort_unstable();
            let background = opaque_lumas[(opaque_lumas.len() * 3 / 4).min(opaque_lumas.len() - 1)];
            if background <= DARK_BACKGROUND_LUMA_MAX {
                return RowInkProfile {
                    has_content: true,
                    artifact_like: false,
                };
            }
            samples
                .iter()
                .enumerate()
                .filter_map(|(index, pixel)| {
                    (pixel[3] > TRANSPARENT_ALPHA
                        && background.saturating_sub(luma(pixel)) >= LUMA_GRADIENT)
                        .then_some(index)
                })
                .collect::<Vec<_>>()
        };
        let active_count = active_indices.len();
        let has_content = active_count * MIN_DARK_OCCUPANCY_DENOMINATOR
            >= samples.len() * MIN_DARK_OCCUPANCY_NUMERATOR;
        if !has_content {
            return RowInkProfile::default();
        }
        let outer_band = ((samples.len() * OUTER_ARTIFACT_BAND_NUMERATOR)
            .div_ceil(OUTER_ARTIFACT_BAND_DENOMINATOR))
        .max(1);
        let confined_to_outer_bands = active_indices
            .iter()
            .all(|&index| index < outer_band || index >= samples.len().saturating_sub(outer_band));
        let first = *active_indices.first().unwrap();
        let last = *active_indices.last().unwrap();
        let span = last - first + 1;
        let dense_edge_band = active_count * DENSE_EDGE_ARTIFACT_DENOMINATOR
            >= samples.len() * DENSE_EDGE_ARTIFACT_NUMERATOR;
        let sparse_and_wide = span * WIDE_ARTIFACT_SPAN_DENOMINATOR
            >= samples.len() * WIDE_ARTIFACT_SPAN_NUMERATOR
            && active_count * MAX_WIDE_ARTIFACT_OCCUPANCY_DENOMINATOR
                <= samples.len() * MAX_WIDE_ARTIFACT_OCCUPANCY_NUMERATOR;
        RowInkProfile {
            has_content,
            artifact_like: confined_to_outer_bands || dense_edge_band || sparse_and_wide,
        }
    }

    fn reference_detect_rgba(
        pixels: &[u8],
        stride: usize,
        width: usize,
        height: usize,
        spread: bool,
    ) -> Option<CropRect> {
        if width == 0
            || height == 0
            || stride < width.checked_mul(CHANNELS)?
            || pixels.len() < stride.checked_mul(height)?
        {
            return None;
        }
        let sample_step =
            ((width.max(height) + MAX_SAMPLE_DIMENSION - 1) / MAX_SAMPLE_DIMENSION).max(1);
        let sample_height = height.div_ceil(sample_step);
        if sample_height < MIN_CONSECUTIVE_CONTENT_LINES {
            return None;
        }
        let regions = if spread && width >= 2 {
            vec![(0, width / 2), (width / 2, width)]
        } else {
            vec![(0, width)]
        };
        let rows = (0..sample_height)
            .map(|sample_y| {
                let y_start = sample_y * sample_step;
                let y_end = (y_start + sample_step).min(height);
                let mut combined = RowInkProfile::default();
                let mut all_content_is_artifact_like = true;
                for &(x_start, x_end) in &regions {
                    let mut samples = Vec::new();
                    for x in (x_start..x_end).step_by(sample_step) {
                        samples.push(reference_average_block_rgba(
                            pixels,
                            stride,
                            x,
                            (x + sample_step).min(x_end),
                            y_start,
                            y_end,
                        ));
                    }
                    let profile = reference_row_ink_profile(&samples);
                    if profile.has_content {
                        combined.has_content = true;
                        all_content_is_artifact_like &= profile.artifact_like;
                    }
                }
                combined.artifact_like = combined.has_content && all_content_is_artifact_like;
                combined
            })
            .collect::<Vec<_>>();

        let top = first_content_with_edge_artifact_guard(&rows)? * sample_step;
        let bottom = last_content_with_edge_artifact_guard(&rows)? * sample_step;
        let padding = (width.max(height) / SAFETY_PADDING_DIVISOR)
            .clamp(SAFETY_PADDING_MIN, SAFETY_PADDING_MAX);
        let y = top.saturating_sub(padding);
        let bottom = (bottom + sample_step + padding).min(height);
        let crop = CropRect {
            x: 0,
            y,
            width,
            height: bottom.checked_sub(y)?,
        };
        if crop == CropRect::full(width, height)
            || crop.height < MIN_RETAINED_PIXELS.min(height)
            || crop.height * MIN_RETAINED_FRACTION_DENOMINATOR < height
        {
            return None;
        }
        Some(crop)
    }

    #[test]
    fn optimized_block_average_matches_the_previous_channel_loop() {
        let (width, height, padding) = (37, 29, 12);
        let stride = width * CHANNELS + padding;
        let mut value = 0x1234_5678_u32;
        let mut pixels = vec![0; stride * height];
        for byte in &mut pixels {
            value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *byte = (value >> 24) as u8;
        }
        for &(x_start, x_end, y_start, y_end) in &[
            (0, 37, 0, 29),
            (3, 17, 5, 21),
            (36, 37, 28, 29),
            (10, 10, 7, 20),
        ] {
            assert_eq!(
                average_block_rgba(&pixels, stride, x_start, x_end, y_start, y_end),
                reference_average_block_rgba(&pixels, stride, x_start, x_end, y_start, y_end)
            );
        }
    }

    #[test]
    fn histogram_profile_matches_the_previous_sorted_implementation() {
        let mut value = 0x9e37_79b9_u32;
        for len in [1, 2, 3, 7, 10, 31, 80, 319, 640] {
            for variant in 0..32 {
                let mut samples = Vec::with_capacity(len);
                for index in 0..len {
                    value = value.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let alpha = if (index + variant) % 11 == 0 {
                        (value >> 27) as u8
                    } else {
                        255
                    };
                    samples.push([
                        (value >> 24) as u8,
                        (value >> 16) as u8,
                        (value >> 8) as u8,
                        alpha,
                    ]);
                }
                assert_eq!(
                    row_ink_profile(&samples),
                    reference_row_ink_profile(&samples),
                    "len={len}, variant={variant}"
                );
            }
        }
    }

    #[test]
    fn cancellable_detector_stops_without_producing_a_crop() {
        let pixels = image(1200, 1800, [255, 255, 255, 255]);
        let mut checks = 0;
        let result = detect_rgba_cancellable(&pixels, 1200 * CHANNELS, 1200, 1800, false, || {
            checks += 1;
            checks >= 2
        });
        assert!(matches!(result, DetectResult::Cancelled));
    }

    #[test]
    fn optimized_detector_matches_the_previous_implementation_at_manga_resolution() {
        for &(width, height, spread, transparent) in &[
            (1280, 1920, false, false),
            (1800, 1200, true, false),
            (901, 1403, false, true),
        ] {
            let background = if transparent {
                [0, 0, 0, 0]
            } else {
                [247, 245, 242, 255]
            };
            let mut pixels = image(width, height, background);
            let margin = height / 10;
            for y in margin..height - margin {
                for x in (width / 10..width - width / 10).step_by(7) {
                    let tone = ((x * 17 + y * 29 + (x ^ y)) % 110) as u8;
                    set_pixel(&mut pixels, width, x, y, [tone, tone, tone, 255]);
                }
            }
            assert_eq!(
                detect_rgba(&pixels, width * CHANNELS, width, height, spread),
                reference_detect_rgba(&pixels, width * CHANNELS, width, height, spread),
                "{width}x{height}, spread={spread}, transparent={transparent}"
            );
        }
    }

    #[test]
    fn crops_only_top_and_bottom_white_margins() {
        let (width, height) = (80, 70);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 15, 12, 50, 45, [20, 20, 20, 255]);
        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert_eq!(crop.x, 0);
        assert_eq!(crop.width, width);
        assert!(crop.y > 0);
        assert!(crop.y + crop.height < height);
    }

    #[test]
    fn tolerates_off_white_noise_and_isolated_speckles() {
        let (width, height) = (80, 70);
        let mut pixels = image(width, height, [244, 242, 239, 255]);
        for y in (1..10).step_by(3) {
            set_pixel(&mut pixels, width, y, y, [210, 208, 205, 255]);
        }
        fill_rect(&mut pixels, width, 16, 13, 48, 42, [30, 30, 30, 255]);
        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert_eq!(crop.x, 0);
        assert_eq!(crop.width, width);
        assert!(crop.y >= 9);
    }

    #[test]
    fn skips_short_dense_scan_band_before_real_content() {
        let (width, height) = (100, 200);
        let mut pixels = image(width, height, [250, 250, 250, 255]);
        for y in 0..3 {
            for x in (0..width).step_by(2) {
                set_pixel(&mut pixels, width, x, y, [40, 40, 40, 255]);
            }
        }
        fill_rect(&mut pixels, width, 20, 20, 60, 150, [20, 20, 20, 255]);

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert!(crop.y > 3);
        assert!(crop.y < 20);
    }

    #[test]
    fn skips_short_corner_noise_before_real_content() {
        let (width, height) = (100, 200);
        let mut pixels = image(width, height, [250, 250, 250, 255]);
        fill_rect(&mut pixels, width, 0, 0, 5, 3, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 95, 0, 5, 3, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 20, 20, 60, 150, [20, 20, 20, 255]);

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert!(crop.y > 3);
        assert!(crop.y < 20);
    }

    #[test]
    fn spread_protects_small_interior_content_on_only_one_page() {
        let (width, height) = (200, 200);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 130, 0, 5, 12, [20, 20, 20, 255]);
        for x in (20..180).step_by(5) {
            fill_rect(&mut pixels, width, x, 25, 1, 150, [20, 20, 20, 255]);
        }

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, true).unwrap();
        assert_eq!(crop.y, 0);
    }

    #[test]
    fn spread_keeps_full_height_when_one_page_has_a_dark_background() {
        let (width, height) = (200, 200);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 20, 20, 60, 160, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 100, 0, 100, 200, [0, 0, 0, 255]);
        fill_rect(&mut pixels, width, 125, 20, 50, 20, [255, 255, 255, 255]);

        assert_eq!(
            detect_rgba(&pixels, width * CHANNELS, width, height, true),
            None
        );
    }

    #[test]
    fn spread_corner_noise_is_evaluated_per_logical_page() {
        let (width, height) = (200, 200);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 0, 0, 5, 3, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 95, 0, 5, 3, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 100, 0, 5, 3, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 195, 0, 5, 3, [20, 20, 20, 255]);
        for x in (20..180).step_by(5) {
            fill_rect(&mut pixels, width, x, 20, 1, 150, [20, 20, 20, 255]);
        }

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, true).unwrap();
        assert!(crop.y > 3);
        assert!(crop.y < 20);
    }

    #[test]
    fn bottom_can_drop_small_page_numbers() {
        let (width, height) = (100, 100);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 20, 10, 60, 65, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 92, 88, 2, 3, [20, 20, 20, 255]);

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert!(crop.y + crop.height < 88);
    }

    #[test]
    fn keeps_substantial_bottom_content() {
        let (width, height) = (100, 100);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 20, 10, 60, 65, [20, 20, 20, 255]);
        fill_rect(&mut pixels, width, 45, 95, 10, 5, [20, 20, 20, 255]);

        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert_eq!(crop.y + crop.height, height);
    }

    #[test]
    fn uniform_and_invalid_images_fall_back() {
        let pixels = image(80, 70, [128, 128, 128, 255]);
        assert_eq!(detect_rgba(&pixels, 80 * CHANNELS, 80, 70, false), None);
        assert_eq!(detect_rgba(&pixels, 1, 80, 70, false), None);
    }

    #[test]
    fn suspiciously_small_crop_falls_back() {
        let (width, height) = (80, 70);
        let mut pixels = image(width, height, [255, 255, 255, 255]);
        fill_rect(&mut pixels, width, 35, 30, 10, 10, [20, 20, 20, 255]);
        assert_eq!(
            detect_rgba(&pixels, width * CHANNELS, width, height, false),
            None
        );
    }

    #[test]
    fn transparent_margin_and_safety_padding_are_preserved() {
        let (width, height) = (80, 70);
        let mut pixels = image(width, height, [0, 0, 0, 0]);
        fill_rect(&mut pixels, width, 15, 12, 50, 45, [30, 30, 30, 255]);
        let crop = detect_rgba(&pixels, width * CHANNELS, width, height, false).unwrap();
        assert_eq!(crop.x, 0);
        assert_eq!(crop.width, width);
        assert!(crop.y < 12 && crop.height < height);
    }
}
