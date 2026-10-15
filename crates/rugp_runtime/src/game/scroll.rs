//! Portable image-area scroll commands; content panning is approximated.
use super::*;

pub(super) fn apply(
    object: &mut ObjectState,
    class: &str,
    fields: BTreeMap<String, Value>,
) -> Result<()> {
    let x = number(&fields, "水平距離", 0)?;
    let y = number(&fields, "垂直距離", 0)?;
    let duration = number(&fields, "時間基", 0)?;
    ensure!(
        number(&fields, "モード", 0)? == 0 && number(&fields, "同期属性", 0)? == 0,
        "unsupported scroll mode/synchronization attributes"
    );
    ensure!(duration >= 0, "negative scroll duration");
    let prefix = match class {
        "OM_BodyScroll" => {
            // Helper 99 configures/resets an independent content origin before
            // reuse. Retain it, but show the whole portable source surface
            // instead of cropping/panning within its native body viewport.
            "render.body_scroll"
        }
        "OM_BoxScroll" => {
            // Approximate helper 99's absolute repositioning with an instant
            // endpoint. Its separate blocking CVmBlt retains the transition
            // duration; this command does not present or change any VM wait.
            object.rect.0 = x;
            object.rect.1 = y;
            "render.box_scroll"
        }
        _ => bail!("unsupported scroll command {class}"),
    };
    for (key, value) in fields {
        object.properties.insert(format!("{prefix}.{key}"), value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fields(x: i32, y: i32, duration: i32) -> BTreeMap<String, Value> {
        BTreeMap::from([
            ("水平距離".into(), Value::Integer(x)),
            ("垂直距離".into(), Value::Integer(y)),
            ("時間基".into(), Value::Integer(duration)),
        ])
    }
    #[test]
    fn body_reset_and_box_endpoint_are_distinct_and_preserve_dimensions() -> Result<()> {
        let mut object = ObjectState {
            rect: (11, 12, 40, 30),
            ..ObjectState::default()
        };
        apply(&mut object, "OM_BodyScroll", fields(0, 0, 0))?;
        assert_eq!(object.rect, (11, 12, 40, 30));
        apply(&mut object, "OM_BodyScroll", fields(100, -20, 400))?;
        assert_eq!(object.rect, (11, 12, 40, 30));
        assert!(matches!(
            object.properties["render.body_scroll.水平距離"],
            Value::Integer(100)
        ));
        apply(&mut object, "OM_BoxScroll", fields(1024, -50, 1000))?;
        assert_eq!(object.rect, (1024, -50, 40, 30));
        assert!(matches!(
            object.properties["render.box_scroll.時間基"],
            Value::Integer(1000)
        ));
        apply(&mut object, "OM_BoxScroll", fields(0, 0, 0))?;
        assert_eq!(object.rect, (0, 0, 40, 30)); // absolute, not relative
        assert_eq!(object.camera, Camera::default());
        assert!(object.visible && object.layers.is_empty());
        Ok(())
    }
    #[test]
    fn unsupported_scrolls_do_not_partially_move_the_surface() {
        let mut object = ObjectState {
            rect: (11, 12, 40, 30),
            ..ObjectState::default()
        };
        for (class, packet) in [
            ("OM_BodyScroll", fields(1, 0, -1)),
            ("OM_BoxScroll", fields(100, 100, -1)),
            ("other", fields(100, 100, 0)),
            (
                "OM_BoxScroll",
                BTreeMap::from([("水平距離".into(), Value::Null)]),
            ),
            (
                "OM_BoxScroll",
                BTreeMap::from([("モード".into(), Value::Integer(1))]),
            ),
            (
                "OM_BoxScroll",
                BTreeMap::from([("同期属性".into(), Value::Integer(1))]),
            ),
        ] {
            assert!(apply(&mut object, class, packet).is_err());
            assert_eq!(object.rect, (11, 12, 40, 30));
            assert!(object.properties.is_empty());
        }
    }
    #[test]
    fn box_endpoint_moves_real_pixels_and_body_pan_remains_a_documented_approximation() -> Result<()>
    {
        let mut object = ObjectState {
            rect: (0, 0, 2, 2),
            ..ObjectState::default()
        };
        object.layers.insert(
            0,
            Layer::Screen(Arc::new(Texture {
                width: 2,
                height: 2,
                display_scale: 1.0,
                rgba: [255, 0, 0, 255].repeat(4),
            })),
        );
        let render = |object: &ObjectState| -> Result<Framebuffer> {
            let mut frame = Framebuffer::new(4, 4)?;
            object.render(&mut frame)?;
            Ok(frame)
        };
        let before = render(&object)?;
        apply(&mut object, "OM_BodyScroll", fields(100, -20, 400))?;
        assert_eq!(render(&object)?.rgba, before.rgba);
        apply(&mut object, "OM_BoxScroll", fields(2, 2, 400))?;
        let after = render(&object)?;
        assert_eq!(&before.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&after.rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&after.rgba[40..44], &[255, 0, 0, 255]);
        Ok(())
    }
}
