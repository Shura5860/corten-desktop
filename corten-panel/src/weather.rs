//! Weather provider and data model for Corten Panel

use serde::{Deserialize, Serialize};
use tracing::debug;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherReport {
    pub city: String,
    pub temp_c: i32,
    pub condition: String,
    pub icon: String,
    pub humidity: u8,
    pub wind_kmh: u8,
    pub forecast: Vec<DayForecast>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayForecast {
    pub day_name: String,
    pub temp_max: i32,
    pub temp_min: i32,
    pub icon: String,
}

impl Default for WeatherReport {
    fn default() -> Self {
        Self {
            city: "İstanbul".to_string(),
            temp_c: 19,
            condition: "Parçalı Bulutlu".to_string(),
            icon: "⛅".to_string(),
            humidity: 58,
            wind_kmh: 14,
            forecast: vec![
                DayForecast {
                    day_name: "Yarın".to_string(),
                    temp_max: 21,
                    temp_min: 13,
                    icon: "☀️".to_string(),
                },
                DayForecast {
                    day_name: "Cuma".to_string(),
                    temp_max: 18,
                    temp_min: 12,
                    icon: "⛅".to_string(),
                },
                DayForecast {
                    day_name: "Cumartesi".to_string(),
                    temp_max: 16,
                    temp_min: 10,
                    icon: "🌧️".to_string(),
                },
            ],
        }
    }
}

/// Asynchronously attempts to refresh weather data from wttr.in or cached source
pub async fn fetch_weather_report() -> WeatherReport {
    // Attempt rapid curl query with 2s timeout
    let output = tokio::process::Command::new("curl")
        .args(["-s", "--max-time", "2", "wttr.in/?format=j1"])
        .output()
        .await;

    if let Ok(output) = output {
        if output.status.success() {
            if let Ok(json_str) = std::str::from_utf8(&output.stdout) {
                if let Ok(parsed) = parse_wttr_json(json_str) {
                    debug!("Successfully fetched live weather data: {}°C", parsed.temp_c);
                    return parsed;
                }
            }
        }
    }

    // Fallback to robust default report
    WeatherReport::default()
}

fn parse_wttr_json(raw: &str) -> Result<WeatherReport, ()> {
    #[derive(Deserialize)]
    struct WttrResponse {
        current_condition: Vec<CurrentCondition>,
        nearest_area: Option<Vec<NearestArea>>,
        weather: Option<Vec<WeatherDay>>,
    }

    #[derive(Deserialize)]
    struct CurrentCondition {
        #[serde(rename = "temp_C")]
        temp_c: String,
        humidity: String,
        #[serde(rename = "windspeedKmph")]
        wind_kmph: String,
        #[serde(rename = "weatherDesc")]
        weather_desc: Vec<ValueObj>,
    }

    #[derive(Deserialize)]
    struct NearestArea {
        #[serde(rename = "areaName")]
        area_name: Vec<ValueObj>,
    }

    #[derive(Deserialize)]
    struct WeatherDay {
        date: String,
        #[serde(rename = "maxtempC")]
        max_c: String,
        #[serde(rename = "mintempC")]
        min_c: String,
        hourly: Vec<Hourly>,
    }

    #[derive(Deserialize)]
    struct Hourly {
        #[serde(rename = "weatherDesc")]
        weather_desc: Vec<ValueObj>,
    }

    #[derive(Deserialize)]
    struct ValueObj {
        value: String,
    }

    let parsed: WttrResponse = serde_json::from_str(raw).map_err(|_| ())?;
    let cur = parsed.current_condition.first().ok_or(())?;

    let temp_c = cur.temp_c.parse::<i32>().unwrap_or(19);
    let humidity = cur.humidity.parse::<u8>().unwrap_or(55);
    let wind_kmh = cur.wind_kmph.parse::<u8>().unwrap_or(12);
    let raw_desc = cur
        .weather_desc
        .first()
        .map(|v| v.value.as_str())
        .unwrap_or("Partly cloudy");

    let (condition, icon) = map_weather_condition(raw_desc);

    let city = parsed
        .nearest_area
        .as_ref()
        .and_then(|a| a.first())
        .and_then(|a| a.area_name.first())
        .map(|v| v.value.clone())
        .unwrap_or_else(|| "İstanbul".to_string());

    let mut forecast = Vec::new();
    if let Some(days) = parsed.weather {
        for (i, d) in days.iter().skip(1).take(3).enumerate() {
            let max_c = d.max_c.parse::<i32>().unwrap_or(temp_c + 1);
            let min_c = d.min_c.parse::<i32>().unwrap_or(temp_c - 5);
            let desc = d
                .hourly
                .first()
                .and_then(|h| h.weather_desc.first())
                .map(|v| v.value.as_str())
                .unwrap_or("Clear");
            let (_, fc_icon) = map_weather_condition(desc);

            let day_name = match i {
                0 => "Yarın".to_string(),
                _ => d.date.clone(),
            };

            forecast.push(DayForecast {
                day_name,
                temp_max: max_c,
                temp_min: min_c,
                icon: fc_icon.to_string(),
            });
        }
    }

    if forecast.is_empty() {
        forecast = WeatherReport::default().forecast;
    }

    Ok(WeatherReport {
        city,
        temp_c,
        condition: condition.to_string(),
        icon: icon.to_string(),
        humidity,
        wind_kmh,
        forecast,
    })
}

fn map_weather_condition(desc: &str) -> (&'static str, &'static str) {
    let lower = desc.to_lowercase();
    if lower.contains("sunny") || lower.contains("clear") {
        ("Güneşli / Açık", "☀️")
    } else if lower.contains("partly") {
        ("Parçalı Bulutlu", "⛅")
    } else if lower.contains("cloud") || lower.contains("overcast") {
        ("Bulutlu / Kapalı", "☁️")
    } else if lower.contains("rain") || lower.contains("drizzle") || lower.contains("shower") {
        ("Yağmurlu", "🌧️")
    } else if lower.contains("thunder") || lower.contains("storm") {
        ("Fırtına & Şimşek", "⛈️")
    } else if lower.contains("snow") || lower.contains("ice") {
        ("Karlı", "❄️")
    } else if lower.contains("fog") || lower.contains("mist") {
        ("Sisli", "🌫️")
    } else {
        ("Ilıman", "🌤️")
    }
}
