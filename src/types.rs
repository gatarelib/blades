// Blades  Copyright (C) 2020  Maroš Grego
//
// This file is part of Blades. This program comes with ABSOLUTELY NO WARRANTY;
// This is free software, and you are welcome to redistribute it under the
// conditions of the GNU General Public License version 3.0.
//
// You should have received a copy of the GNU General Public License
// along with Blades.  If not, see <http://www.gnu.org/licenses/>

use crate::config::{Config, TEMPLATE_DIR};
use crate::error::{Error, Result};

use beef::lean::Cow;
use chrono::{DateTime as CDateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Timelike};
use ramhorns::encoding::Encoder;
use ramhorns::traits::ContentSequence;
use ramhorns::{Content, Ramhorns, Section, Template};
use serde::de::{self, Deserialize, Deserializer, Visitor};

use std::collections::HashSet;
use std::fmt;
use std::path::{is_separator, Path, PathBuf};
use std::time::SystemTime;

pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, ahash::RandomState>;
/// A set of all the rendered paths. Behind a mutex, so it can be written from multiple threads.
pub type MutSet<T = PathBuf> = parking_lot::Mutex<HashSet<T, ahash::RandomState>>;

/// Aggregation of all the templets of the site's theme and its template dir.
pub struct Templates {
    templates: Option<Ramhorns>,
    theme: Option<Ramhorns>,
}

/// A wrapper around the `choron::NaiveDateTime`, used for rendering of dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime(NaiveDateTime);

/// A wrapper around a `str` representing path, used to derive `Content` implementation
/// that acts like an iterator over the path segmets.
#[derive(serde::Deserialize)]
pub(crate) struct Ancestors<'a>(#[serde(borrow)] Cow<'a, str>);

/// One segment of a path.
#[derive(Content)]
struct Segment<'a>(
    /// This segment.
    #[ramhorns(rename = "name")]
    &'a str,
    /// Full path up to this segment.
    #[ramhorns(rename = "full")]
    &'a str,
);

/// A sum of all the types that can be used in a TOML file.
#[derive(Clone, serde::Deserialize)]
#[serde(untagged)]
pub(crate) enum Any<'a> {
    Number(f64),
    #[serde(borrow)]
    String(Cow<'a, str>),
    DateTime(DateTime),
    #[serde(borrow)]
    List(Vec<Any<'a>>),
    #[serde(borrow)]
    Map(HashMap<Cow<'a, str>, Any<'a>>),
}

impl Templates {
    /// Load the templates from the directories specified by the config.
    #[inline]
    pub fn load(config: &Config) -> Result<Self, Error> {
        Ok(Self {
            templates: if Path::new(TEMPLATE_DIR).exists() {
                Some(Ramhorns::from_folder(TEMPLATE_DIR)?)
            } else {
                None
            },
            theme: if !config.theme.is_empty() {
                let mut theme_path =
                    Path::new(config.theme_dir.as_ref()).join(config.theme.as_ref());
                theme_path.push(TEMPLATE_DIR);
                if theme_path.exists() {
                    Some(Ramhorns::from_folder(theme_path)?)
                } else {
                    None
                }
            } else {
                None
            },
        })
    }

    /// Get one template with the given name or return an error.
    #[inline]
    pub fn get(&self, name: &str) -> Result<&Template<'static>, Error> {
        self.templates
            .as_ref()
            .and_then(|t| t.get(name))
            .or_else(|| self.theme.as_ref().and_then(|t| t.get(name)))
            .ok_or_else(|| Error::MissingTemplate { name: name.into() })
    }
}

impl<'a> Content for Ancestors<'a> {
    #[inline]
    fn is_truthy(&self) -> bool {
        !self.0.is_empty()
    }

    #[inline]
    fn render_escaped<E: Encoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        // The path was stripped of leading separators.
        if !self.0.is_empty() {
            encoder.write_unescaped("/")?;
            encoder.write_escaped(&self.0)?;
        }
        Ok(())
    }

    #[inline]
    fn render_unescaped<E: Encoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        if !self.0.is_empty() {
            encoder.write_unescaped("/")?;
            encoder.write_unescaped(&self.0)?;
        }
        Ok(())
    }

    #[inline]
    fn render_section<C, E>(&self, section: Section<C>, encoder: &mut E) -> Result<(), E::Error>
    where
        C: ContentSequence,
        E: Encoder,
    {
        let s = self.0.as_ref();
        if s.is_empty() {
            return Ok(());
        }

        let mut previous = 0;
        for (i, sep) in s.match_indices(is_separator) {
            section
                .with(&Segment(&s[previous..i], &s[0..i]))
                .render(encoder)?;
            previous = i + sep.len();
        }
        if !s.contains(is_separator) {
            section.with(&Segment(s, s)).render(encoder)?;
        }
        Ok(())
    }
}

impl AsRef<str> for Ancestors<'_> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl<'a> Default for Ancestors<'a> {
    #[inline]
    fn default() -> Self {
        Ancestors(Cow::const_str(""))
    }
}

impl<'a> From<Cow<'a, str>> for Ancestors<'a> {
    #[inline]
    fn from(s: Cow<'a, str>) -> Self {
        Ancestors(s)
    }
}

impl<'a> Content for Any<'a> {
    #[inline]
    fn is_truthy(&self) -> bool {
        match self {
            Any::List(vec) => !vec.is_empty(),
            Any::Map(map) => !map.is_empty(),
            _ => false,
        }
    }

    #[inline]
    fn render_escaped<E: Encoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        match self {
            Any::Number(n) => n.render_escaped(encoder),
            Any::String(s) => s.render_escaped(encoder),
            Any::DateTime(dt) => dt.render_escaped(encoder),
            Any::List(vec) => vec.render_escaped(encoder),
            Any::Map(map) => map.render_escaped(encoder),
        }
    }

    #[inline]
    fn render_unescaped<E: Encoder>(&self, encoder: &mut E) -> Result<(), E::Error> {
        match self {
            Any::Number(n) => n.render_unescaped(encoder),
            Any::String(s) => s.render_unescaped(encoder),
            Any::DateTime(dt) => dt.render_unescaped(encoder),
            Any::List(vec) => vec.render_unescaped(encoder),
            Any::Map(map) => map.render_unescaped(encoder),
        }
    }

    #[inline]
    fn render_section<C, E>(&self, section: Section<C>, encoder: &mut E) -> Result<(), E::Error>
    where
        C: ContentSequence,
        E: Encoder,
    {
        match self {
            Any::List(vec) => vec.render_section(section, encoder),
            Any::Map(map) => map.render_section(section, encoder),
            _ => section.render(encoder),
        }
    }

    #[inline]
    fn render_field_escaped<E>(&self, h: u64, name: &str, enc: &mut E) -> Result<bool, E::Error>
    where
        E: Encoder,
    {
        match self {
            Any::Map(map) => map.render_field_escaped(h, name, enc),
            _ => Ok(false),
        }
    }

    #[inline]
    fn render_field_unescaped<E>(&self, h: u64, name: &str, enc: &mut E) -> Result<bool, E::Error>
    where
        E: Encoder,
    {
        match self {
            Any::Map(map) => map.render_field_unescaped(h, name, enc),
            _ => Ok(false),
        }
    }

    #[inline]
    fn render_field_section<C, E>(
        &self,
        hash: u64,
        name: &str,
        section: Section<C>,
        encoder: &mut E,
    ) -> Result<bool, E::Error>
    where
        C: ContentSequence,
        E: Encoder,
    {
        match self {
            Any::Map(map) => map.render_field_section(hash, name, section, encoder),
            _ => Ok(false),
        }
    }

    #[inline]
    fn render_field_inverse<C, E>(
        &self,
        hash: u64,
        name: &str,
        section: Section<C>,
        encoder: &mut E,
    ) -> Result<bool, E::Error>
    where
        C: ContentSequence,
        E: Encoder,
    {
        match self {
            Any::Map(map) => map.render_field_inverse(hash, name, section, encoder),
            _ => Ok(false),
        }
    }
}

impl DateTime {
    pub fn now() -> Self {
        SystemTime::now().into()
    }
}

impl Content for DateTime {
    #[inline]
    fn render_section<C, E>(&self, section: Section<C>, encoder: &mut E) -> Result<(), E::Error>
    where
        C: ContentSequence,
        E: Encoder,
    {
        section.with(self).render(encoder)
    }

    #[inline]
    fn render_field_escaped<E>(&self, _: u64, name: &str, enc: &mut E) -> Result<bool, E::Error>
    where
        E: Encoder,
    {
        if name.len() != 1 {
            return Ok(false);
        }

        const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        const NUMS: [&str; 60] = [
            "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13",
            "14", "15", "16", "17", "18", "19", "20", "21", "22", "23", "24", "25", "26", "27",
            "28", "29", "30", "31", "32", "33", "34", "35", "36", "37", "38", "39", "40", "41",
            "42", "43", "44", "45", "46", "47", "48", "49", "50", "51", "52", "53", "54", "55",
            "56", "57", "58", "59",
        ];

        match name.bytes().next().unwrap_or(0) {
            b'y' => self.0.year().render_unescaped(enc).map(|_| true),
            b'm' => enc
                .write_unescaped(NUMS[self.0.month() as usize])
                .map(|_| true),
            b'd' => enc
                .write_unescaped(NUMS[self.0.day() as usize])
                .map(|_| true),
            b'e' => self.0.day().render_unescaped(enc).map(|_| true),
            b'H' => enc
                .write_unescaped(NUMS[self.0.hour() as usize])
                .map(|_| true),
            b'M' => enc
                .write_unescaped(NUMS[self.0.minute() as usize])
                .map(|_| true),
            b'S' => enc
                .write_unescaped(NUMS[self.0.second() as usize])
                .map(|_| true),
            b'a' => enc
                .write_unescaped(WEEKDAYS[self.0.weekday().num_days_from_sunday() as usize])
                .map(|_| true),
            b'b' => enc
                .write_unescaped(MONTHS[self.0.month0() as usize])
                .map(|_| true),
            _ => Ok(false),
        }
    }

    #[inline]
    fn render_field_unescaped<E>(&self, h: u64, name: &str, enc: &mut E) -> Result<bool, E::Error>
    where
        E: Encoder,
    {
        self.render_field_escaped(h, name, enc)
    }
}

// Toml crate currently doesn't supprot deserializing dates into types other than String,
// so an ugly hack based on its `Deserializer` private fields needs to be used.
const FIELD: &str = "$__toml_private_datetime";
const NAME: &str = "$__toml_private_Datetime";

impl<'de> Deserialize<'de> for DateTime {
    fn deserialize<D>(deserializer: D) -> Result<DateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct DateTimeKey;
        struct DateTimeVisitor;

        impl<'de> Deserialize<'de> for DateTimeKey {
            fn deserialize<D>(deserializer: D) -> Result<DateTimeKey, D::Error>
            where
                D: de::Deserializer<'de>,
            {
                struct FieldVisitor;

                impl<'de> de::Visitor<'de> for FieldVisitor {
                    type Value = ();

                    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                        formatter.write_str("a valid datetime field")
                    }

                    fn visit_str<E>(self, s: &str) -> Result<(), E>
                    where
                        E: de::Error,
                    {
                        if s == FIELD {
                            Ok(())
                        } else {
                            Err(de::Error::custom("expected field with a custom name"))
                        }
                    }
                }

                deserializer.deserialize_identifier(FieldVisitor)?;
                Ok(DateTimeKey)
            }
        }

        impl<'de> Visitor<'de> for DateTimeVisitor {
            type Value = DateTime;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a TOML datetime")
            }

            fn visit_map<V>(self, mut visitor: V) -> Result<DateTime, V::Error>
            where
                V: de::MapAccess<'de>,
            {
                let value = visitor.next_key::<DateTimeKey>()?;
                if value.is_none() {
                    return Err(de::Error::custom("datetime key not found"));
                }
                let v: &str = visitor.next_value()?;
                v.parse::<NaiveDateTime>()
                    .or_else(|_| v.parse::<NaiveDate>().map(|d| d.and_hms(0, 0, 0)))
                    .or_else(|_| NaiveDateTime::parse_from_str(v, "%F %T%.f"))
                    .or_else(|_| v.parse::<CDateTime<FixedOffset>>().map(|d| d.naive_utc()))
                    .map(DateTime)
                    .map_err(|_| {
                        de::Error::custom(format!("unable to parse date and time from {}", v))
                    })
            }
        }

        static FIELDS: [&str; 1] = [FIELD];
        deserializer.deserialize_struct(NAME, &FIELDS, DateTimeVisitor)
    }
}

impl From<SystemTime> for DateTime {
    fn from(st: SystemTime) -> Self {
        let time: chrono::DateTime<chrono::Utc> = st.into();
        DateTime(time.naive_utc())
    }
}
