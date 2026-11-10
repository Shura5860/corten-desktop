use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeatherData {
    pub location: String,
    pub temperature: f32,
    pub condition: String,
    pub icon: String,
    pub humidity: u8,
    pub wind_speed: f32,
}

impl Default for WeatherData {
    fn default() -> Self {
        Self {
            location: "İstanbul".to_string(),
            temperature: 19.5,
            condition: "Güneşli".to_string(),
            icon: "☀️".to_string(),
            humidity: 62,
            wind_speed: 14.0,
        }
    }
}

pub struct WeatherService;

impl WeatherService {
    fn cache_path() -> PathBuf {
        if let Some(dirs) = ProjectDirs::from("org", "corten", "corten-desktop") {
            let dir = dirs.cache_dir();
            let _ = fs::create_dir_all(dir);
            dir.join("weather_cache.json")
        } else {
            PathBuf::from("/tmp/corten_weather_cache.json")
        }
    }

    pub fn load_cached_or_default() -> WeatherData {
        let path = Self::cache_path();
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(data) = serde_json::from_str::<WeatherData>(&content) {
                return data;
            }
        }
        let default_data = WeatherData::default();
        Self::save_cache(&default_data);
        default_data
    }

    pub fn save_cache(data: &WeatherData) {
        let path = Self::cache_path();
        if let Ok(json) = serde_json::to_string_pretty(data) {
            let _ = fs::write(path, json);
        }
    }

    /// Fetches live weather asynchronously via wttr.in or falls back to cache
    pub async fn fetch_weather_async() -> WeatherData {
        let output = tokio::process::Command::new("curl")
            .args(["-s", "--max-time", "2", "wttr.in/?format=j1"])
            .output()
            .await;

        if let Ok(output) = output {
            if output.status.success() {
                if let Ok(json_str) = std::str::from_utf8(&output.stdout) {
                    if let Ok(data) = Self::parse_wttr(json_str) {
                        Self::save_cache(&data);
                        return data;
                    }
                }
            }
        }

        Self::load_cached_or_default()
    }

    fn parse_wttr(raw: &str) -> Result<WeatherData, ()> {
        #[derive(Deserialize)]
        struct WttrResponse {
            current_condition: Vec<CurrentCondition>,
            nearest_area: Option<Vec<NearestArea>>,
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
        struct ValueObj {
            value: String,
        }

        let parsed: WttrResponse = serde_json::from_str(raw).map_err(|_| ())?;
        let cur = parsed.current_condition.first().ok_or(())?;

        let temp = cur.temp_c.parse::<f32>().unwrap_or(19.0);
        let humidity = cur.humidity.parse::<u8>().unwrap_or(60);
        let wind = cur.wind_kmph.parse::<f32>().unwrap_or(10.0);

        let desc = cur
            .weather_desc
            .first()
            .map(|v| v.value.as_str())
            .unwrap_or("Clear");

        let location = parsed
            .nearest_area
            .and_then(|a| a.first().and_then(|n| n.area_name.first().map(|v| v.value.clone())))
            .unwrap_or_else(|| "İstanbul".to_string());

        let (condition, icon) = map_condition(desc);

        Ok(WeatherData {
            location,
            temperature: temp,
            condition,
            icon,
            humidity,
            wind_speed: wind,
        })
    }
}

fn map_condition(desc: &str) -> (String, String) {
    let lower = desc.to_lowercase();
    if lower.contains("sunny") || lower.contains("clear") {
        ("Güneşli".to_string(), "☀️".to_string())
    } else if lower.contains("partly cloudy") {
        ("Parçalı Bulutlu".to_string(), "⛅".to_string())
    } else if lower.contains("cloudy") || lower.contains("overcast") {
        ("Bulutlu".to_string(), "☁️".to_string())
    } else if lower.contains("rain") || lower.contains("drizzle") || lower.contains("shower") {
        ("Yağmurlu".to_string(), "🌧️".to_string())
    } else if lower.contains("snow") || lower.contains("blizzard") {
        ("Karlı".to_string(), "❄️".to_string())
    } else if lower.contains("thunder") || lower.contains("storm") {
        ("Fırtınalı".to_string(), "⛈️".to_string())
    } else if lower.contains("fog") || lower.contains("mist") {
        ("Sisli".to_string(), "🌫️".to_string())
    } else {
        (desc.to_string(), "⛅".to_string())
    }
}
