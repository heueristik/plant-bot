use crate::location;
use crate::location::Location;
use chrono::{DateTime, Timelike, Utc};
use open_meteo_api::models::OpenMeteoData;
use open_meteo_api::query::OpenMeteo;
use uom::si::area::square_meter;
use uom::si::f32::{Area, Length, Volume};
use uom::si::length::millimeter;
use uom::si::volume::liter;

pub async fn query_weather_data(location: Location) -> OpenMeteoData {
    OpenMeteo::new()
        .coordinates(location.latitude, location.longitude)
        .unwrap()
        .forecast_days(2)
        .unwrap()
        .current_weather()
        .unwrap()
        .past_days(2)
        .unwrap()
        .time_zone(location.time_zone)
        .unwrap()
        .hourly()
        .unwrap()
        .daily()
        .unwrap()
        .query()
        .await
        .unwrap()
}

pub async fn calculate_cycles_needed(data: &OpenMeteoData) -> usize {
    let area = Area::new::<square_meter>(0.1); // Example: Big planter
    let delta = Length::new::<millimeter>(precipitation_evaporation_delta(data));
    let volume = (delta * area).abs();

    println!(
        "{} volume (24h): {:.2} L",
        if delta.value > 0.0 {
            "Surplus"
        } else {
            "Deficient"
        },
        volume.get::<liter>()
    );

    if delta.value > 0.0 {
        0 // No cycles needed
    } else {
        let volume_per_cycle = Volume::new::<liter>(0.12);
        let cycles = (volume / volume_per_cycle).value.ceil() as usize;

        println!("Watering cycles needed: {cycles}");
        cycles
    }
}

pub fn calculate_cycles_needed_blocked() -> usize {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let data = query_weather_data(location::BERLIN).await;
        calculate_cycles_needed(&data).await
    })
}

fn precipitation_evaporation_delta(data: &OpenMeteoData) -> f32 {
    let index = find_current_hourly_index(data).expect("Current hourly index not found");
    let hourly_data = data.hourly.as_ref().unwrap();

    let precipitation = calculate_metric(&hourly_data.precipitation, index, -24);
    let evapotranspiration = calculate_metric(&hourly_data.et0_fao_evapotranspiration, index, -24);

    let delta = precipitation - evapotranspiration;
    println!(
        "Δ = Precipitation - Evapotranspiration: {precipitation}mm - {evapotranspiration}mm = {delta}mm"
    );

    delta
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
async fn test_find_current_hour_index() {
    let data = query_weather_data(location::BERLIN).await;

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
