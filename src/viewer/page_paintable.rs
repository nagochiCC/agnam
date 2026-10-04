use super::smart_crop::CropRect;
use gtk::{gdk, glib, graphene, prelude::*, subclass::prelude::*};
use std::cell::OnceCell;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PagePaintable {
        pub(super) data: OnceCell<(gdk::Texture, CropRect)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PagePaintable {
        const NAME: &'static str = "AgnamPagePaintable";
        type Type = super::PagePaintable;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for PagePaintable {}

    impl gdk::subclass::prelude::PaintableImpl for PagePaintable {
        fn flags(&self) -> gdk::PaintableFlags {
            gdk::PaintableFlags::SIZE | gdk::PaintableFlags::CONTENTS
        }

        fn intrinsic_width(&self) -> i32 {
            self.data.get().map_or(0, |(_, crop)| crop.width as i32)
        }

        fn intrinsic_height(&self) -> i32 {
            self.data.get().map_or(0, |(_, crop)| crop.height as i32)
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            self.data
                .get()
                .map_or(0.0, |(_, crop)| crop.width as f64 / crop.height as f64)
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            let Some((texture, crop)) = self.data.get() else {
                return;
            };
            let Some(snapshot) = snapshot.downcast_ref::<gtk::Snapshot>() else {
                return;
            };
            if width <= 0.0 || height <= 0.0 {
                return;
            }
            let sx = width / crop.width as f64;
            let sy = height / crop.height as f64;
            let bounds = [
                -(crop.x as f64 * sx) as f32,
                -(crop.y as f64 * sy) as f32,
                (texture.width() as f64 * sx) as f32,
                (texture.height() as f64 * sy) as f32,
            ];
            if !bounds.iter().all(|value| value.is_finite()) {
                return;
            }
            snapshot.push_clip(&graphene::Rect::new(0.0, 0.0, width as f32, height as f32));
            snapshot.append_texture(
                texture,
                &graphene::Rect::new(bounds[0], bounds[1], bounds[2], bounds[3]),
            );
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct PagePaintable(ObjectSubclass<imp::PagePaintable>) @implements gdk::Paintable;
}

impl PagePaintable {
    pub(super) fn new(texture: &gdk::Texture, crop: CropRect) -> Self {
        let page: Self = glib::Object::new();
        page.imp().data.set((texture.clone(), crop)).unwrap();
        page
    }

    #[cfg(test)]
    pub(super) fn texture(&self) -> gdk::Texture {
        self.imp().data.get().unwrap().0.clone()
    }

    #[cfg(test)]
    pub(super) fn region(&self) -> CropRect {
        self.imp().data.get().unwrap().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Snapshot is a plain GObject; constructing it does not need a display or
    // GTK widgets. Exercise the actual Paintable snapshot through GSK/Cairo.
    fn snapshot_node(
        page: &impl IsA<gdk::Paintable>,
        width: f64,
        height: f64,
    ) -> Option<gtk::gsk::RenderNode> {
        let snapshot: gtk::Snapshot = glib::Object::new();
        page.snapshot(&snapshot, width, height);
        snapshot.to_node()
    }

    #[test]
    fn snapshot_maps_crop_coordinates_and_clips_at_uniform_and_nonuniform_scales() {
        let pixels: Vec<u8> = (0..6)
            .flat_map(|y| (0..9).flat_map(move |x| [x * 20, y * 30, 90, 255]))
            .collect();
        let texture: gdk::Texture = gdk::MemoryTexture::new(
            9,
            6,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(pixels),
            9 * 4,
        )
        .upcast();
        // Single vertical crop and the two halves of an odd-width Spread.
        for crop in [
            CropRect {
                x: 0,
                y: 1,
                width: 9,
                height: 4,
            },
            CropRect {
                x: 4,
                y: 1,
                width: 5,
                height: 4,
            },
            CropRect {
                x: 0,
                y: 1,
                width: 4,
                height: 4,
            },
        ] {
            let page = PagePaintable::new(&texture, crop);
            assert_eq!(page.intrinsic_width(), crop.width as i32);
            assert_eq!(page.intrinsic_height(), crop.height as i32);
            assert_eq!(
                page.intrinsic_aspect_ratio(),
                crop.width as f64 / crop.height as f64
            );
            for (sx, sy) in [(1, 1), (2, 2), (3, 2)] {
                let width = crop.width * sx;
                let height = crop.height * sy;
                let node = snapshot_node(&page, width as f64, height as f64).unwrap();
                assert_eq!(
                    node.bounds(),
                    graphene::Rect::new(0.0, 0.0, width as f32, height as f32)
                );
                let mut surface = gtk::cairo::ImageSurface::create(
                    gtk::cairo::Format::ARgb32,
                    (width + 4) as i32,
                    (height + 4) as i32,
                )
                .unwrap();
                let context = gtk::cairo::Context::new(&surface).unwrap();
                context.translate(2.0, 2.0);
                node.draw(&context);
                drop(context);
                let stride = surface.stride() as usize;
                let data = surface.data().unwrap();
                let pixel = |x: usize, y: usize| {
                    let start = y * stride + x * 4;
                    u32::from_ne_bytes(data[start..start + 4].try_into().unwrap())
                };
                for y in 0..height + 4 {
                    for x in 0..width + 4 {
                        if !(2..width + 2).contains(&x) || !(2..height + 2).contains(&y) {
                            assert_eq!(pixel(x, y), 0, "pixels outside the crop must be clipped");
                        }
                    }
                }
                for y in 0..crop.height {
                    for x in 0..crop.width {
                        let value = pixel(2 + x * sx + sx / 2, 2 + y * sy + sy / 2);
                        // At 1:1 every source pixel must match. At larger scales
                        // inspect interior samples; filtering can blend edges.
                        if sx == 1 {
                            assert_eq!((value >> 16) & 255, ((crop.x + x) * 20) as u32);
                            assert_eq!((value >> 8) & 255, ((crop.y + y) * 30) as u32);
                        } else {
                            assert!(
                                ((value >> 16) & 255).abs_diff(((crop.x + x) * 20) as u32) <= 6
                            );
                            assert!(((value >> 8) & 255).abs_diff(((crop.y + y) * 30) as u32) <= 8);
                        }
                        assert_eq!(value >> 24, 255);
                    }
                }
            }
            for (width, height) in [
                (0.0, 4.0),
                (9.0, 0.0),
                (f64::INFINITY, 4.0),
                (f64::MAX, 4.0),
            ] {
                assert!(snapshot_node(&page, width, height).is_none());
            }
        }
    }
}
