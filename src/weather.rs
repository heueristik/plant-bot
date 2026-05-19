use crate::location::Location;
use chrono::{DateTime, Timelike, Utc};
use open_meteo_api::models::OpenMeteoData;
use open_meteo_api::query::OpenMeteo;
use uom::ConversionFactor;
use uom::si::area::square_meter;
use uom::si::f32::{Area, Length, Volume};
use uom::si::length::millimeter;
use uom::si::volume::liter;

pub async fn query_weather_data(
    location: Location,
) -> Result<OpenMeteoData, Box<dyn std::error::Error>> {
    OpenMeteo::new()
        .coordinates(location.latitude, location.longitude)?
        .forecast_days(2)?
        .current_weather()?
        .past_days(2)?
        .time_zone(location.time_zone)?
        .hourly()?
        .daily()?
        .query()
        .await
}

pub fn calculate_cycles_needed_blocked(
    location: Location,
) -> Result<usize, Box<dyn std::error::Error>> {
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    rt.block_on(async {
        let data = query_weather_data(location).await?;
        let estimate = estimate_watering(&data)?;
        print_estimate(&estimate);
        Ok(estimate.cycles)
    })
}

/// The outcome of the watering calculation for one location.
struct WateringEstimate {
    /// Rainfall over the past 24 h.
    precipitation: Length,
    /// Evapotranspiration over the past 24 h.
    evapotranspiration: Length,
    /// The watered area.
    area: Area,
    /// Net water budget — positive is a surplus, negative a deficit.
    volume: Volume,
    /// Number of watering cycles needed.
    cycles: usize,
}

/// Computes how many watering cycles a location needs from its forecast.
///
/// Returns an error if the forecast lacks the hourly data for the current hour.
fn estimate_watering(data: &OpenMeteoData) -> Result<WateringEstimate, Box<dyn std::error::Error>> {
    let index =
        find_current_hourly_index(data).ok_or("current hour not found in the weather forecast")?;
    let hourly = data
        .hourly
        .as_ref()
        .ok_or("weather forecast contains no hourly data")?;

    let precipitation =
        Length::new::<millimeter>(calculate_metric(&hourly.precipitation, index, -24));
    let evapotranspiration =
        Length::new::<millimeter>(calculate_metric(&hourly.et0_fao_evapotranspiration, index, -24));
    let delta = precipitation - evapotranspiration;

    let area = Area::new::<square_meter>(0.5);
    let scale_factor = 2.0;
    let volume = scale_factor * delta * area;

    let cycles = if delta.value > 0.0 {
        0 // surplus — no watering needed
    } else {
        let volume_per_cycle = Volume::new::<liter>(0.5);
        let needed = (volume.abs() / volume_per_cycle).value.ceil() as usize;
        needed.min(10)
    };

    Ok(WateringEstimate {
        precipitation,
        evapotranspiration,
        area,
        volume,
        cycles,
    })
}

/// Prints a human-readable summary of a watering estimate.
fn print_estimate(estimate: &WateringEstimate) {
    println!(
        "     Precipitation (24h): {:.2} mm",
        estimate.precipitation.get::<millimeter>().value()
    );
    println!(
        "Evapotranspiration (24h): {:.2} mm",
        estimate.evapotranspiration.get::<millimeter>().value()
    );
    println!(
        "                   Area : {:.2} m2",
        estimate.area.get::<square_meter>().value()
    );

    let label = if estimate.volume.value > 0.0 {
        "          Surplus"
    } else {
        "        Deficient"
    };
    println!(
        "{label} volume: {:.2} L\n",
        estimate.volume.get::<liter>().value().abs()
    );
}

fn calculate_metric(data: &[Option<f32>], index: usize, range_hours: isize) -> f32 {
    let range = if range_hours.is_negative() {
        index.saturating_sub(range_hours.unsigned_abs())..=index
    } else {
        index..=(index + range_hours.unsigned_abs())
    };

    data[range].iter().map(|&value| value.unwrap_or(0.0)).sum()
}

fn find_current_hourly_index(data: &OpenMeteoData) -> Option<usize> {
    let current_time = Utc::now()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();

    data.hourly.as_ref()?.time.iter().position(|timestamp| {
        DateTime::parse_from_rfc3339(&adjusted_date_with_offset(
            timestamp,
            data.utc_offset_seconds,
        ))
        .is_ok_and(|parsed| parsed == current_time)
    })
}

fn adjusted_date_with_offset(date: &str, utc_offset_seconds: f32) -> String {
    format!("{}{}", date, utc_offset_string(utc_offset_seconds))
}

fn utc_offset_string(utc_offset_seconds: f32) -> String {
    let hours = (utc_offset_seconds as i32) / 3600;
    let minutes = ((utc_offset_seconds as i32) % 3600) / 60;

    format!(":00{:+03}:{:02}", hours, minutes.abs())
}

#[tokio::test]
#[ignore = "requires network access to the Open-Meteo API"]
async fn test_find_current_hour_index() {
    use crate::location::BERLIN;

    let data = query_weather_data(BERLIN).await.unwrap();

    let index = find_current_hourly_index(&data).unwrap();
    let found_time_parsing = DateTime::parse_from_rfc3339(&adjusted_date_with_offset(
        &data.hourly.as_ref().unwrap().time[index],
        data.utc_offset_seconds,
    ))
    .unwrap();

    let current_time = Utc::now()
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap();

    assert_eq!(found_time_parsing, current_time);
}
