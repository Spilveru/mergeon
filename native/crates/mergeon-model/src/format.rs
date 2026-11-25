//! Formatos de fecha y tamaño (`src/lib/format.js`).
//!
//! El original se apoya en `Date` e `Intl`. Aquí las salidas de `en-US` están
//! escritas a mano y comprobadas contra el Electron de la app (ICU 78):
//!
//! - `format_relative_date` más allá de una semana: `Mar 9, 2026, 08:05 PM`.
//! - `format_date_time`: `03/09/2026, 8:05 PM`.
//! - `format_short_date`: `Jan 5, 24`.
//!
//! Diferencias inevitables frente a `Intl`:
//!
//! - `formatShortDate` usaba el locale del sistema (`undefined`). Aquí siempre
//!   sale en `en-US`, como el resto de la interfaz: con otro locale del sistema
//!   el original daba, por ejemplo, `5 ene 24`.
//! - `new Date(texto)` acepta en V8 muchos formatos heredados. Aquí se leen los
//!   que la app produce de verdad (ISO 8601 de `git log`/GitHub, `%ai` de git,
//!   RFC 2822 y la fecha por defecto de git); cualquier otro da `Invalid Date`,
//!   igual que una fecha ilegible en el original.
//! - La zona horaria es la del `DateTime`/`TimeZone` que se pase; las variantes
//!   sin sufijo usan la zona local del sistema, como el navegador.

use chrono::{
    DateTime, Datelike, Days, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    TimeZone, Timelike, Utc,
};
use regex::Regex;
use std::sync::LazyLock;

use crate::jsstr::js_round;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 86_400_000;

const INVALID_DATE: &str = "Invalid Date";

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Lo que el original le pasa a `new Date(...)`: un texto (fechas de git y de
/// GitHub) o milisegundos desde la época (blame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateInput<'a> {
    Text(&'a str),
    EpochMs(i64),
}

impl<'a> From<&'a str> for DateInput<'a> {
    fn from(text: &'a str) -> Self {
        DateInput::Text(text)
    }
}

impl<'a> From<&'a String> for DateInput<'a> {
    fn from(text: &'a String) -> Self {
        DateInput::Text(text)
    }
}

impl From<i64> for DateInput<'_> {
    fn from(epoch_ms: i64) -> Self {
        DateInput::EpochMs(epoch_ms)
    }
}

impl<'a> From<Option<&'a str>> for DateInput<'a> {
    fn from(text: Option<&'a str>) -> Self {
        DateInput::Text(text.unwrap_or_default())
    }
}

impl DateInput<'_> {
    /// `!dateString`: texto vacío o el número 0.
    fn is_falsy(self) -> bool {
        match self {
            DateInput::Text(text) => text.is_empty(),
            DateInput::EpochMs(epoch_ms) => epoch_ms == 0,
        }
    }

    fn epoch_ms<Tz: TimeZone>(self, local: &Tz) -> Option<i64> {
        match self {
            DateInput::Text(text) => parse_js_date(text, local),
            DateInput::EpochMs(epoch_ms) => Some(epoch_ms),
        }
    }
}

static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([0-9]{4})-([0-9]{2})-([0-9]{2})(?:[T ]([0-9]{2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]+))?)?(?: ?(Z|[+-][0-9]{2}:?[0-9]{2}))?)?$",
    )
    .expect("iso date regex")
});

fn parse_offset(text: &str) -> Option<FixedOffset> {
    if text == "Z" {
        return FixedOffset::east_opt(0);
    }

    let sign = if text.starts_with('-') { -1 } else { 1 };
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    let hours: i32 = digits.get(..2)?.parse().ok()?;
    let minutes: i32 = digits.get(2..4)?.parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}

fn local_to_epoch_ms<Tz: TimeZone>(naive: &NaiveDateTime, local: &Tz) -> i64 {
    match local.from_local_datetime(naive).earliest() {
        Some(date) => date.timestamp_millis(),
        // Hora que no existe (salto de horario de verano): se toma el
        // desfase vigente alrededor de ese instante.
        None => {
            let offset = local.offset_from_utc_datetime(naive).fix();
            naive.and_utc().timestamp_millis() - i64::from(offset.local_minus_utc()) * 1000
        }
    }
}

/// Milisegundos desde la época de un texto de fecha, como `new Date(text)`.
/// `None` equivale a `Invalid Date`. Un texto sin zona se toma en hora `local`,
/// salvo la fecha sola (`2026-03-09`), que es medianoche UTC como en JavaScript.
pub fn parse_js_date<Tz: TimeZone>(text: &str, local: &Tz) -> Option<i64> {
    if let Some(captures) = ISO_DATE.captures(text) {
        let number = |index: usize| -> Option<u32> {
            captures
                .get(index)
                .and_then(|found| found.as_str().parse().ok())
        };
        let year: i32 = captures[1].parse().ok()?;
        let month = number(2)?;
        let day = number(3)?;
        if !(1..=31).contains(&day) {
            return None;
        }
        // V8 deja desbordar el día dentro de 1..31 (30 de febrero → 2 de marzo).
        let date = NaiveDate::from_ymd_opt(year, month, 1)?
            .checked_add_days(Days::new(u64::from(day - 1)))?;

        let Some(hour) = number(4) else {
            return Some(date.and_time(NaiveTime::MIN).and_utc().timestamp_millis());
        };
        let minute = number(5)?;
        let second = number(6).unwrap_or(0);
        // JavaScript solo conserva milisegundos: lo demás se trunca.
        let millis = captures.get(7).map_or(0, |fraction| {
            let digits = format!("{:0<3}", fraction.as_str());
            digits[..3].parse::<u32>().unwrap_or(0)
        });
        let naive = date.and_time(NaiveTime::from_hms_milli_opt(hour, minute, second, millis)?);

        return match captures.get(8) {
            Some(offset) => {
                let offset = parse_offset(offset.as_str())?;
                Some(
                    naive.and_utc().timestamp_millis() - i64::from(offset.local_minus_utc()) * 1000,
                )
            }
            None => Some(local_to_epoch_ms(&naive, local)),
        };
    }

    if let Ok(date) = DateTime::parse_from_rfc2822(text) {
        return Some(date.timestamp_millis());
    }

    // Fecha por defecto de git: `Mon Mar 9 10:00:00 2026 -0400`.
    DateTime::parse_from_str(text, "%a %b %e %H:%M:%S %Y %z")
        .ok()
        .map(|date| date.timestamp_millis())
}

struct Fields {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
}

impl Fields {
    fn at<Tz: TimeZone>(epoch_ms: i64, zone: &Tz) -> Option<Self> {
        let date = zone.timestamp_millis_opt(epoch_ms).single()?;
        Some(Self {
            year: date.year(),
            month: date.month(),
            day: date.day(),
            hour: date.hour(),
            minute: date.minute(),
        })
    }

    fn month_name(&self) -> &'static str {
        MONTHS[(self.month as usize).saturating_sub(1) % 12]
    }

    fn hour12(&self) -> u32 {
        match self.hour % 12 {
            0 => 12,
            hour => hour,
        }
    }

    fn day_period(&self) -> &'static str {
        if self.hour < 12 { "AM" } else { "PM" }
    }
}

fn plural(count: i64) -> &'static str {
    if count != 1 { "s" } else { "" }
}

/// "5 minutes ago" / "3 hours ago" / "2 days ago", full date+time beyond a week.
pub fn format_relative_date<'a>(date: impl Into<DateInput<'a>>) -> String {
    format_relative_date_at(date, &Local::now())
}

/// [`format_relative_date`] con el "ahora" (y su zona horaria) explícitos.
pub fn format_relative_date_at<'a, Tz: TimeZone>(
    date: impl Into<DateInput<'a>>,
    now: &DateTime<Tz>,
) -> String {
    let zone = now.timezone();
    let Some(date_ms) = date.into().epoch_ms(&zone) else {
        return INVALID_DATE.to_string();
    };

    let diff_ms = now.timestamp_millis() - date_ms;
    let diff_mins = diff_ms.div_euclid(MINUTE_MS);
    let diff_hours = diff_ms.div_euclid(HOUR_MS);
    let diff_days = diff_ms.div_euclid(DAY_MS);

    if diff_mins < 60 {
        return format!("{diff_mins} minute{} ago", plural(diff_mins));
    }

    if diff_hours < 24 {
        return format!("{diff_hours} hour{} ago", plural(diff_hours));
    }

    if diff_days < 7 {
        return format!("{diff_days} day{} ago", plural(diff_days));
    }

    match Fields::at(date_ms, &zone) {
        Some(fields) => format!(
            "{} {}, {}, {:02}:{:02} {}",
            fields.month_name(),
            fields.day,
            fields.year,
            fields.hour12(),
            fields.minute,
            fields.day_period()
        ),
        None => INVALID_DATE.to_string(),
    }
}

/// Compact date for dense rows, e.g. "Jan 5, 24".
pub fn format_short_date<'a>(date: impl Into<DateInput<'a>>) -> String {
    format_short_date_in(date, &Local)
}

/// [`format_short_date`] en una zona horaria explícita.
pub fn format_short_date_in<'a, Tz: TimeZone>(date: impl Into<DateInput<'a>>, zone: &Tz) -> String {
    let date = date.into();
    if date.is_falsy() {
        return String::new();
    }

    match date
        .epoch_ms(zone)
        .and_then(|epoch_ms| Fields::at(epoch_ms, zone))
    {
        Some(fields) => format!(
            "{} {}, {:02}",
            fields.month_name(),
            fields.day,
            fields.year.rem_euclid(100)
        ),
        None => INVALID_DATE.to_string(),
    }
}

/// Full date + time, "—" when missing.
pub fn format_date_time<'a>(date: impl Into<DateInput<'a>>) -> String {
    format_date_time_in(date, &Local)
}

/// [`format_date_time`] en una zona horaria explícita.
pub fn format_date_time_in<'a, Tz: TimeZone>(date: impl Into<DateInput<'a>>, zone: &Tz) -> String {
    let date = date.into();
    if date.is_falsy() {
        return "—".to_string();
    }

    match date
        .epoch_ms(zone)
        .and_then(|epoch_ms| Fields::at(epoch_ms, zone))
    {
        Some(fields) => format!(
            "{:02}/{:02}/{}, {}:{:02} {}",
            fields.month,
            fields.day,
            fields.year,
            fields.hour12(),
            fields.minute,
            fields.day_period()
        ),
        None => INVALID_DATE.to_string(),
    }
}

/// Coarse "Updated X ago" copy for repository listings.
pub fn format_updated_ago<'a>(date: impl Into<DateInput<'a>>) -> String {
    format_updated_ago_at(date, &Local::now())
}

/// [`format_updated_ago`] con el "ahora" explícito.
pub fn format_updated_ago_at<'a, Tz: TimeZone>(
    date: impl Into<DateInput<'a>>,
    now: &DateTime<Tz>,
) -> String {
    let Some(date_ms) = date.into().epoch_ms(&now.timezone()) else {
        // En el original todas las comparaciones con NaN fallan y cae al último caso.
        return "Updated NaN years ago".to_string();
    };
    let diff_days = (now.timestamp_millis() - date_ms).div_euclid(DAY_MS);

    if diff_days == 0 {
        return "Updated today".to_string();
    }
    if diff_days == 1 {
        return "Updated yesterday".to_string();
    }
    if diff_days < 7 {
        return format!("Updated {diff_days} days ago");
    }
    if diff_days < 30 {
        return format!("Updated {} weeks ago", diff_days / 7);
    }
    if diff_days < 365 {
        return format!("Updated {} months ago", diff_days / 30);
    }
    format!("Updated {} years ago", diff_days / 365)
}

/// `value.toFixed(1)`. Rust redondea los empates al par y JavaScript hacia
/// arriba; un empate exacto en el primer decimal solo ocurre en múltiplos
/// impares de 0,25, que se tratan aparte.
fn to_fixed_1(value: f64) -> String {
    let quarters = value * 4.0;
    if quarters.fract() == 0.0 && quarters.rem_euclid(2.0) == 1.0 {
        return format!("{:.1}", (value * 10.0 + 0.5).floor() / 10.0);
    }

    format!("{value:.1}")
}

/// Tamaño de archivo en la unidad que lo deja legible: "512 B", "2.9 KB",
/// "29 KB" (un decimal solo por debajo de 10).
///
/// Recibe `f64` como el `Number` del original (`size as f64` desde un entero);
/// un valor no finito o negativo da texto vacío.
pub fn format_size(bytes: f64) -> String {
    if !bytes.is_finite() || bytes < 0.0 {
        return String::new();
    }

    if bytes < 1024.0 {
        return format!("{bytes} B");
    }

    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    let mut value = bytes / 1024.0;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if value < 10.0 {
        format!("{} {}", to_fixed_1(value), UNITS[unit])
    } else {
        format!("{} {}", js_round(value), UNITS[unit])
    }
}

/// `new Date().toISOString()`: instante actual en UTC con milisegundos.
pub fn now_iso_string() -> String {
    to_iso_string(Utc::now().timestamp_millis())
}

/// `new Date(epochMs).toISOString()`.
pub fn to_iso_string(epoch_ms: i64) -> String {
    match Utc.timestamp_millis_opt(epoch_ms).single() {
        Some(date) => date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        None => INVALID_DATE.to_string(),
    }
}

/// `Date.now()`.
pub fn now_epoch_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn santiago_winter() -> FixedOffset {
        FixedOffset::west_opt(4 * 3600).unwrap()
    }

    #[test]
    fn reads_the_dates_the_app_produces() {
        let zone = santiago_winter();
        let expected = utc("2026-03-09T14:00:00Z").timestamp_millis();

        // `%aI` de git log, GitHub, `%ai`, RFC 2822 y la fecha por defecto de git.
        assert_eq!(
            parse_js_date("2026-03-09T10:00:00-04:00", &zone),
            Some(expected)
        );
        assert_eq!(parse_js_date("2026-03-09T14:00:00Z", &zone), Some(expected));
        assert_eq!(
            parse_js_date("2026-03-09T14:00:00.000Z", &zone),
            Some(expected)
        );
        assert_eq!(
            parse_js_date("2026-03-09 10:00:00 -0400", &zone),
            Some(expected)
        );
        assert_eq!(
            parse_js_date("2026-03-09T18:00:00+0400", &zone),
            Some(expected)
        );
        assert_eq!(parse_js_date("2026-03-09T14:00Z", &zone), Some(expected));
        assert_eq!(
            parse_js_date("Mon, 9 Mar 2026 10:00:00 -0400", &zone),
            Some(expected)
        );
        assert_eq!(
            parse_js_date("Mon Mar 9 10:00:00 2026 -0400", &zone),
            Some(expected)
        );
        // Sin zona: hora local.
        assert_eq!(parse_js_date("2026-03-09T10:00:00", &zone), Some(expected));
        assert_eq!(parse_js_date("2026-03-09 10:00:00", &zone), Some(expected));
        // Solo fecha: medianoche UTC, como en JavaScript.
        assert_eq!(
            parse_js_date("2026-03-09", &zone),
            Some(utc("2026-03-09T00:00:00Z").timestamp_millis())
        );
        // Los microsegundos se truncan a milisegundos.
        assert_eq!(
            parse_js_date("2026-03-09T10:00:00.123456Z", &zone),
            Some(utc("2026-03-09T10:00:00.123Z").timestamp_millis())
        );
    }

    #[test]
    fn rejects_what_javascript_rejects() {
        let zone = Utc;
        assert_eq!(parse_js_date("", &zone), None);
        assert_eq!(parse_js_date("garbage", &zone), None);
        assert_eq!(parse_js_date("1741500000", &zone), None);
        assert_eq!(parse_js_date(" 2026-03-09T10:00:00Z ", &zone), None);
        assert_eq!(parse_js_date("2026-13-09", &zone), None);
    }

    #[test]
    fn counts_minutes_hours_and_days() {
        let now = utc("2026-03-09T12:00:00Z");

        assert_eq!(
            format_relative_date_at("2026-03-09T12:00:00Z", &now),
            "0 minutes ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-09T11:59:00Z", &now),
            "1 minute ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-09T11:01:00Z", &now),
            "59 minutes ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-09T11:00:00Z", &now),
            "1 hour ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-08T12:00:01Z", &now),
            "23 hours ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-08T12:00:00Z", &now),
            "1 day ago"
        );
        assert_eq!(
            format_relative_date_at("2026-03-02T12:00:01Z", &now),
            "6 days ago"
        );
    }

    #[test]
    fn spells_out_the_date_beyond_a_week() {
        let now = utc("2026-12-31T00:00:00Z");

        // Salidas de `toLocaleDateString('en-US', {...})` en el Electron de la app.
        assert_eq!(
            format_relative_date_at("2026-03-09T00:05:00.000Z", &now),
            "Mar 9, 2026, 12:05 AM"
        );
        assert_eq!(
            format_relative_date_at("2026-07-04T12:30:00Z", &now),
            "Jul 4, 2026, 12:30 PM"
        );
        assert_eq!(
            format_relative_date_at("2024-01-05T12:00:00Z", &now),
            "Jan 5, 2024, 12:00 PM"
        );

        let late = utc("2027-06-01T00:00:00Z");
        assert_eq!(
            format_relative_date_at("2026-12-25T13:45:59-04:00", &late),
            "Dec 25, 2026, 05:45 PM"
        );
    }

    #[test]
    fn shows_dates_in_the_zone_of_the_viewer() {
        let now = utc("2026-12-31T00:00:00Z").with_timezone(&santiago_winter());

        assert_eq!(
            format_relative_date_at("2026-03-09T00:05:00.000Z", &now),
            "Mar 8, 2026, 08:05 PM"
        );
        assert_eq!(
            format_date_time_in("2026-03-09T00:05:00.000Z", &santiago_winter()),
            "03/08/2026, 8:05 PM"
        );
        assert_eq!(
            format_short_date_in("2026-03-09T00:05:00.000Z", &santiago_winter()),
            "Mar 8, 26"
        );
    }

    // Lo que hace el original con entradas raras, tal cual.
    #[test]
    fn keeps_the_quirks_of_the_original_for_odd_input() {
        let now = utc("2026-03-09T12:00:00Z");

        assert_eq!(format_relative_date_at("garbage", &now), "Invalid Date");
        assert_eq!(format_relative_date_at("", &now), "Invalid Date");
        // Una fecha futura (reloj desfasado) sale en negativo.
        assert_eq!(
            format_relative_date_at("2026-03-09T12:04:30Z", &now),
            "-5 minutes ago"
        );
        assert_eq!(
            format_updated_ago_at("garbage", &now),
            "Updated NaN years ago"
        );
        assert_eq!(
            format_updated_ago_at("2026-03-10T13:00:00Z", &now),
            "Updated -2 days ago"
        );
    }

    #[test]
    fn formats_a_compact_date() {
        assert_eq!(
            format_short_date_in("2024-01-05T12:00:00Z", &Utc),
            "Jan 5, 24"
        );
        assert_eq!(
            format_short_date_in("2026-12-25T13:45:59-04:00", &Utc),
            "Dec 25, 26"
        );
        // Blame entrega milisegundos desde la época.
        assert_eq!(
            format_short_date_in(1_700_000_000_000_i64, &Utc),
            "Nov 14, 23"
        );
        assert_eq!(format_short_date_in("", &Utc), "");
        assert_eq!(format_short_date_in(0_i64, &Utc), "");
        assert_eq!(format_short_date_in(None, &Utc), "");
        assert_eq!(format_short_date_in("nope", &Utc), "Invalid Date");
    }

    #[test]
    fn formats_a_full_date_and_time() {
        assert_eq!(
            format_date_time_in("2026-03-09T00:05:00.000Z", &Utc),
            "03/09/2026, 12:05 AM"
        );
        assert_eq!(
            format_date_time_in("2026-12-25T13:45:59-04:00", &Utc),
            "12/25/2026, 5:45 PM"
        );
        assert_eq!(
            format_date_time_in("2024-01-05T12:00:00Z", &Utc),
            "01/05/2024, 12:00 PM"
        );
        assert_eq!(format_date_time_in("", &Utc), "—");
        assert_eq!(format_date_time_in(None, &Utc), "—");
        assert_eq!(format_date_time_in("nope", &Utc), "Invalid Date");
    }

    #[test]
    fn describes_how_long_ago_a_repository_changed() {
        let now = utc("2026-03-09T12:00:00Z");
        let ago = |days: i64| {
            let date = to_iso_string(now.timestamp_millis() - days * DAY_MS);
            format_updated_ago_at(&date, &now)
        };

        assert_eq!(ago(0), "Updated today");
        assert_eq!(ago(1), "Updated yesterday");
        assert_eq!(ago(6), "Updated 6 days ago");
        assert_eq!(ago(7), "Updated 1 weeks ago");
        assert_eq!(ago(29), "Updated 4 weeks ago");
        assert_eq!(ago(30), "Updated 1 months ago");
        assert_eq!(ago(364), "Updated 12 months ago");
        assert_eq!(ago(365), "Updated 1 years ago");
        assert_eq!(ago(800), "Updated 2 years ago");
    }

    #[test]
    fn formats_sizes_in_a_readable_unit() {
        assert_eq!(format_size(0.0), "0 B");
        assert_eq!(format_size(512.0), "512 B");
        assert_eq!(format_size(1023.0), "1023 B");
        assert_eq!(format_size(1024.0), "1.0 KB");
        assert_eq!(format_size(2_939.0), "2.9 KB");
        // A partir de 10 ya no hay decimales.
        assert_eq!(format_size(29_389.0), "29 KB");
        assert_eq!(format_size(10.0 * 1024.0), "10 KB");
        assert_eq!(format_size(1536.0 * 1024.0), "1.5 MB");
        assert_eq!(format_size(5.0 * 1024.0 * 1024.0 * 1024.0), "5.0 GB");
        // No hay unidad por encima de GB.
        assert_eq!(format_size(4096.0 * 1024.0 * 1024.0 * 1024.0), "4096 GB");
    }

    #[test]
    fn rounds_sizes_like_javascript() {
        // 1280 / 1024 = 1.25 exacto: `toFixed(1)` sube a 1.3 (Rust daría 1.2).
        assert_eq!(format_size(1280.0), "1.3 KB");
        assert_eq!(format_size(2304.0), "2.3 KB");
        // 9.97 KB se queda en un decimal aunque redondee a 10.0.
        assert_eq!(format_size(10_209.0), "10.0 KB");
        // `Math.round(10.5)` es 11.
        assert_eq!(format_size(10_752.0), "11 KB");
        assert_eq!(format_size(512.5), "512.5 B");
    }

    #[test]
    fn has_no_size_for_what_is_not_a_size() {
        assert_eq!(format_size(-1.0), "");
        assert_eq!(format_size(f64::NAN), "");
        assert_eq!(format_size(f64::INFINITY), "");
    }

    #[test]
    fn writes_iso_timestamps_like_to_iso_string() {
        assert_eq!(to_iso_string(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(to_iso_string(100), "1970-01-01T00:00:00.100Z");
        assert!(parse_js_date(&now_iso_string(), &Utc).is_some());
        assert!(now_epoch_ms() > 1_700_000_000_000);
    }
}
