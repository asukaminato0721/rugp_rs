use super::*;
use rugp_assets::profile::Profile;

impl Game {
    pub(super) fn access_profile(
        &mut self,
        target: &ObjectRef,
        class: &str,
        fields: &BTreeMap<String, Value>,
    ) -> Result<()> {
        ensure!(
            target
                .class
                .as_ref()
                .is_some_and(|c| c.name == "CStdb" && c.schema == 1),
            "unsupported profile database class"
        );
        let id = target.offset.context("profile database offset missing")?;
        let profile = self.profiles.load(id, || {
            let size = target.size.context("profile database extent missing")?;
            ensure!(size <= 16 * 1024 * 1024, "oversized profile database");
            Profile::parse_object(&self.archive.read_at(id, size as usize)?)
        })?;
        let text = |name: &str| -> Result<&str> {
            match fields.get(name) {
                Some(Value::Text(s)) => Ok(s),
                _ => bail!("profile field {name} is not text"),
            }
        };
        let key = text("キー")?;
        if class == "OM_AccessWrite" {
            return self.profiles.write_string(id, key, text("値")?);
        }
        let default = text("初期値")?;
        let value = if key == "$_DbTxC$_" && default == "$GAt" {
            profile.text()
        } else {
            profile.read_string(key, default)
        };
        let Some(Value::Reference(destination)) = fields.get("値の取得先") else {
            bail!("profile output destination is not an object reference");
        };
        let result = match destination.class.as_ref().map(|c| c.name.as_str()) {
            Some("CInt" | "CUInt") => Value::Integer(atoi(value)),
            Some("CBool" | "CFlag") => {
                Value::Integer(i32::from(value == "true" || atoi(value) != 0))
            }
            Some("CByte") => Value::Integer(i32::from(atoi(value) as u8)),
            Some("CShort") => Value::Integer(i32::from(atoi(value) as i16)),
            Some("CUShort") => Value::Integer(i32::from(atoi(value) as u16)),
            Some("CStrObj") => Value::Text(value.to_owned()),
            other => bail!("unsupported profile output class {other:?}"),
        };
        self.vm.write_object(destination, result)
    }
}
// The native integer setter uses C atoi: ASCII whitespace, optional sign,
// decimal prefix, and zero if no digits. It does not require the entire string
// to be numeric and does not interpret a leading zero as an octal prefix.
pub(super) fn atoi(text: &str) -> i32 {
    let text = text.trim_start_matches([' ', '\t', '\n', '\r', '\x0b', '\x0c']);
    let (negative, digits) = if let Some(s) = text.strip_prefix('-') {
        (true, s)
    } else {
        (false, text.strip_prefix('+').unwrap_or(text))
    };
    let mut value = 0i32;
    for c in digits.bytes().take_while(u8::is_ascii_digit) {
        value = value.wrapping_mul(10).wrapping_add(i32::from(c - b'0'));
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_decimal_prefix_conversion() {
        for (s, n) in [
            (" \t-42suffix", -42),
            ("+17", 17),
            ("0x10", 0),
            ("012", 12),
            ("garbage", 0),
            ("", 0),
            ("2147483648", i32::MIN),
        ] {
            assert_eq!(atoi(s), n);
        }
    }
    #[test]
    #[ignore = "set RUGP_TEST_GAME to the retail game directory"]
    fn retail_profile_outputs_and_persistent_reopen() -> Result<()> {
        use rugp_assets::object::ClassId;
        let root = std::env::var_os("RUGP_TEST_GAME").context("RUGP_TEST_GAME missing")?;
        let directory = std::env::temp_dir().join(format!(
            "rugp-retail-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let reference = |name: &str, schema: u16, offset: u64, size: u32| ObjectRef {
            class: Some(ClassId {
                name: name.into(),
                schema,
            }),
            cache_index: None,
            flags: 0xc108,
            offset: Some(offset),
            size: Some(size),
            owner: None,
            selector: 0,
            inline: None,
        };
        let database = reference("CStdb", 1, 211235, 4);
        let output = reference("CInt", 0, 213487, 7);
        let fields = |key: &str, default: &str| {
            BTreeMap::from([
                ("キー".into(), Value::Text(key.into())),
                ("初期値".into(), Value::Text(default.into())),
                ("値の取得先".into(), Value::Reference(output.clone())),
            ])
        };
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        let actual_directory = game.user_data_dir().unwrap().to_owned();
        game.access_profile(
            &database,
            "OM_AccessWrite",
            &BTreeMap::from([
                ("キー".into(), Value::Text("port-regression".into())),
                ("値".into(), Value::Text(" -37tail".into())),
            ]),
        )?;
        game.access_profile(
            &database,
            "OM_AccessGetValue",
            &fields("port-regression", "99"),
        )?;
        assert_eq!(game.vm.variables[&213487].integer()?, -37);
        drop(game);
        let mut game = Game::open_persistent(root.as_ref(), &directory)?;
        game.access_profile(
            &database,
            "OM_AccessGetValue",
            &fields("port-regression", "99"),
        )?;
        assert_eq!(game.vm.variables[&213487].integer()?, -37);
        game.access_profile(&database, "OM_AccessGetValue", &fields("missing", "77"))?;
        assert_eq!(game.vm.variables[&213487].integer()?, 77);
        drop(game);
        assert!(Game::open_persistent(root.as_ref(), root.as_ref()).is_err());
        std::fs::remove_file(actual_directory.join(format!("database-{:016x}.txt", 211235)))?;
        std::fs::remove_file(actual_directory.join("profiles.lock"))?;
        std::fs::remove_dir(actual_directory)?;
        std::fs::remove_dir(directory)?;
        Ok(())
    }
}
