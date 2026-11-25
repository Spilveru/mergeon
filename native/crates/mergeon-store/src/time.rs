use std::time::{SystemTime, UNIX_EPOCH};

/// Equivalente a `new Date().toISOString()`.
pub(crate) fn iso_now() -> String {
    iso_timestamp(SystemTime::now())
}

/// `YYYY-MM-DDTHH:mm:ss.sssZ`, siempre en UTC, como `Date#toISOString`.
pub(crate) fn iso_timestamp(time: SystemTime) -> String {
    let total_millis = time
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let millis = total_millis % 1000;
    let total_seconds = (total_millis / 1000) as i64;
    let days = total_seconds.div_euclid(86_400);
    let seconds_of_day = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        seconds_of_day / 3600,
        (seconds_of_day % 3600) / 60,
        seconds_of_day % 60
    )
}

// Días desde 1970-01-01 a fecha civil (algoritmo de Howard Hinnant), para no
// traer una dependencia de fechas solo por este formato.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(millis: u64) -> String {
        iso_timestamp(UNIX_EPOCH + Duration::from_millis(millis))
    }

    #[test]
    fn formats_like_date_to_iso_string() {
        assert_eq!(at(0), "1970-01-01T00:00:00.000Z");
        // new Date('2026-07-26T08:00:00.000Z').getTime()
        assert_eq!(at(1_785_052_800_000), "2026-07-26T08:00:00.000Z");
        // Bisiesto y fin de año: new Date(Date.UTC(2024, 1, 29, 23, 59, 59, 7)).
        assert_eq!(at(1_709_251_199_007), "2024-02-29T23:59:59.007Z");
        assert_eq!(at(1_735_689_599_999), "2024-12-31T23:59:59.999Z");
        assert_eq!(at(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn now_has_the_iso_shape() {
        let now = iso_now();
        assert_eq!(now.len(), 24);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[10..11], "T");
    }
}
