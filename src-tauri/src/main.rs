#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use std::sync::Mutex;
use std::path::PathBuf;
use std::fs;
use sysinfo::{System, Components};
use tokio::net::TcpStream;
use tokio::time::{timeout, Duration};
use chrono::{DateTime, Utc};
use reqwest::Client;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SystemInfo {
    cpu_usage: f32,
    memory_used: u64,
    memory_total: u64,
    disk_available: u64,
    cpu_temp: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct NetworkStats {
    download_speed: f32,
    upload_speed: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ServiceType {
    Web,
    Tcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceConfig {
    id: String,
    name: String,
    service_type: ServiceType,
    host: String,
    port: u16,
    check_interval: u64,
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceStatus {
    service_id: String,
    is_online: bool,
    response_time: u64,
    last_checked: DateTime<Utc>,
    error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServicesData {
    services: Vec<ServiceConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AppConfig {
    temp_threshold_orange: f32,
    temp_threshold_red: f32,
    ha_url: Option<String>,
    ha_token: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            temp_threshold_orange: 60.0,
            temp_threshold_red: 80.0,
            ha_url: None,
            ha_token: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HaEntityState {
    entity_id: String,
    state: String,
    attributes: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct HaServiceCall {
    domain: String,
    service: String,
    entity_id: String,
}

struct AppState {
    system: Mutex<System>,
    components: Mutex<Components>,
    services: Mutex<Vec<ServiceConfig>>,
    config: Mutex<AppConfig>,
    client: Client,
}

impl AppState {
    fn new() -> Self {
        let services = Self::load_services();
        let config = Self::load_config();
        AppState {
            system: Mutex::new(System::new_all()),
            components: Mutex::new(Components::new_with_refreshed_list()),
            services: Mutex::new(services),
            config: Mutex::new(config),
            client: Client::new(),
        }
    }

    fn get_config_dir() -> PathBuf {
        let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push(".monitor-dashboard");
        fs::create_dir_all(&path).ok();
        path
    }

    fn get_services_path() -> PathBuf {
        let mut path = Self::get_config_dir();
        path.push("services.json");
        path
    }

    fn get_config_path() -> PathBuf {
        let mut path = Self::get_config_dir();
        path.push("config.json");
        path
    }

    fn load_services() -> Vec<ServiceConfig> {
        let path = Self::get_services_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(data) = serde_json::from_str::<ServicesData>(&content) {
                    return data.services;
                }
            }
        }
        Vec::new()
    }

    fn load_config() -> AppConfig {
        let path = Self::get_config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                    return config;
                }
            }
        }
        AppConfig::default()
    }

    fn save_services(services: &[ServiceConfig]) -> Result<(), String> {
        let path = Self::get_services_path();
        let data = ServicesData {
            services: services.to_vec(),
        };
        let json = serde_json::to_string_pretty(&data)
            .map_err(|e| format!("序列化失败: {}", e))?;
        fs::write(&path, json)
            .map_err(|e| format!("写入文件失败: {}", e))?;
        Ok(())
    }

    fn save_config(config: &AppConfig) -> Result<(), String> {
        let path = Self::get_config_path();
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| format!("序列化失败: {}", e))?;
        fs::write(&path, json)
            .map_err(|e| format!("写入文件失败: {}", e))?;
        Ok(())
    }
}

async fn check_tcp_connection(host: &str, port: u16, timeout_ms: u64) -> Result<u64, String> {
    let addr = format!("{}:{}", host, port);
    let start = std::time::Instant::now();
    
    match timeout(Duration::from_millis(timeout_ms), TcpStream::connect(&addr)).await {
        Ok(Ok(_)) => {
            let elapsed = start.elapsed().as_millis() as u64;
            Ok(elapsed)
        }
        Ok(Err(e)) => Err(format!("连接失败: {}", e)),
        Err(_) => Err("连接超时".to_string()),
    }
}

async fn check_http_connection(host: &str, port: u16, timeout_ms: u64) -> Result<u64, String> {
    let url = if port == 443 {
        format!("https://{}", host)
    } else if port == 80 {
        format!("http://{}", host)
    } else {
        format!("http://{}:{}", host, port)
    };
    
    let start = std::time::Instant::now();
    
    match timeout(Duration::from_millis(timeout_ms), async {
        let client = Client::builder()
            .timeout(Duration::from_millis(timeout_ms))
            .build()
            .map_err(|e| format!("创建客户端失败: {}", e))?;
        
        let response = client.get(&url).send().await
            .map_err(|e| format!("请求失败: {}", e))?;
        
        if response.status().is_success() || response.status().is_redirection() {
            Ok(())
        } else {
            Err(format!("HTTP 错误: {}", response.status()))
        }
    }).await {
        Ok(Ok(_)) => {
            let elapsed = start.elapsed().as_millis() as u64;
            Ok(elapsed)
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err("请求超时".to_string()),
    }
}

#[tauri::command]
async fn get_system_info(state: tauri::State<'_, AppState>) -> Result<SystemInfo, String> {
    let mut sys = state.system.lock().unwrap();
    sys.refresh_all();
    
    let mut components = state.components.lock().unwrap();
    components.refresh();
    
    let mut cpu_temp = None;
    
    // 尝试查找 CPU 温度
    // 不同系统和硬件的 label 可能不同，这里做一个简单的遍历查找
    for component in components.iter() {
        let label = component.label().to_lowercase();
        let temp = component.temperature();
        
        if temp <= 0.0 { continue; }

        if cpu_temp.is_none() && (label.contains("cpu") || label.contains("core") || label.contains("package") || label.contains("k10temp")) {
            cpu_temp = Some(temp);
            break;
        }
    }

    Ok(SystemInfo {
        cpu_usage: sys.global_cpu_usage(),
        memory_used: sys.used_memory(),
        memory_total: sys.total_memory(),
        disk_available: 2 * 1024 * 1024 * 1024,
        cpu_temp,
    })
}

#[tauri::command]
async fn get_network_stats() -> Result<NetworkStats, String> {
    Ok(NetworkStats {
        download_speed: 840.0,
        upload_speed: 42.0,
    })
}

#[tauri::command]
async fn toggle_device(device: String, state: bool) -> Result<bool, String> {
    println!("Toggle device: {} to {}", device, state);
    Ok(true)
}

#[tauri::command]
async fn get_services(state: tauri::State<'_, AppState>) -> Result<Vec<ServiceConfig>, String> {
    let services = state.services.lock().unwrap();
    Ok(services.clone())
}

#[tauri::command]
async fn add_service(service: ServiceConfig, state: tauri::State<'_, AppState>) -> Result<ServiceConfig, String> {
    let mut services = state.services.lock().unwrap();
    
    if services.iter().any(|s| s.id == service.id) {
        return Err("服务 ID 已存在".to_string());
    }
    
    services.push(service.clone());
    AppState::save_services(&services)?;
    Ok(service)
}

#[tauri::command]
async fn update_service(id: String, service: ServiceConfig, state: tauri::State<'_, AppState>) -> Result<ServiceConfig, String> {
    let mut services = state.services.lock().unwrap();
    
    let index = services.iter().position(|s| s.id == id)
        .ok_or_else(|| "服务不存在".to_string())?;
    
    services[index] = service.clone();
    AppState::save_services(&services)?;
    Ok(service)
}

#[tauri::command]
async fn delete_service(id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let mut services = state.services.lock().unwrap();
    
    let index = services.iter().position(|s| s.id == id)
        .ok_or_else(|| "服务不存在".to_string())?;
    
    services.remove(index);
    AppState::save_services(&services)?;
    Ok(true)
}

#[tauri::command]
async fn check_service(id: String, state: tauri::State<'_, AppState>) -> Result<ServiceStatus, String> {
    let service = {
        let services = state.services.lock().unwrap();
        services.iter().find(|s| s.id == id)
            .ok_or_else(|| "服务不存在".to_string())?
            .clone()
    };
    
    let timeout_ms = 5000;
    let result = match service.service_type {
        ServiceType::Web => check_http_connection(&service.host, service.port, timeout_ms).await,
        ServiceType::Tcp => check_tcp_connection(&service.host, service.port, timeout_ms).await,
    };
    
    let status = match result {
        Ok(response_time) => ServiceStatus {
            service_id: id.clone(),
            is_online: true,
            response_time,
            last_checked: Utc::now(),
            error_message: None,
        },
        Err(error) => ServiceStatus {
            service_id: id.clone(),
            is_online: false,
            response_time: 0,
            last_checked: Utc::now(),
            error_message: Some(error),
        },
    };
    
    Ok(status)
}

#[tauri::command]
async fn get_all_status(state: tauri::State<'_, AppState>) -> Result<Vec<ServiceStatus>, String> {
    let enabled_services: Vec<ServiceConfig> = {
        let services = state.services.lock().unwrap();
        services.iter()
            .filter(|s| s.enabled)
            .cloned()
            .collect()
    };
    
    let mut statuses = Vec::new();
    for service in enabled_services {
        match check_service(service.id.clone(), state.clone()).await {
            Ok(status) => statuses.push(status),
            Err(_) => {
                statuses.push(ServiceStatus {
                    service_id: service.id.clone(),
                    is_online: false,
                    response_time: 0,
                    last_checked: Utc::now(),
                    error_message: Some("检查失败".to_string()),
                });
            }
        }
    }
    
    Ok(statuses)
}

#[tauri::command]
async fn get_app_config(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    let config = state.config.lock().unwrap();
    Ok(config.clone())
}

#[tauri::command]
async fn update_app_config(config: AppConfig, state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    let mut current_config = state.config.lock().unwrap();
    *current_config = config.clone();
    AppState::save_config(&config)?;
    Ok(config)
}

#[tauri::command]
async fn check_ha_connection(url: String, token: String) -> Result<bool, String> {
    let client = Client::new();
    let response = client.get(format!("{}/api/", url))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if response.status().is_success() {
        Ok(true)
    } else {
        Err(format!("连接失败，状态码: {}", response.status()))
    }
}

#[tauri::command]
async fn get_ha_entities(state: tauri::State<'_, AppState>) -> Result<Vec<HaEntityState>, String> {
    let (url, token) = {
        let config = state.config.lock().unwrap();
        let url = config.ha_url.clone().ok_or("未配置 Home Assistant 地址")?;
        let token = config.ha_token.clone().ok_or("未配置 Home Assistant Token")?;
        (url, token)
    };

    let client = &state.client;
    let response = client.get(format!("{}/api/states", url))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if response.status().is_success() {
        let entities: Vec<HaEntityState> = response.json().await
            .map_err(|e| format!("解析响应失败: {}", e))?;
        Ok(entities)
    } else {
        Err(format!("获取状态失败: {}", response.status()))
    }
}

#[tauri::command]
async fn call_ha_service(domain: String, service: String, entity_id: String, state: tauri::State<'_, AppState>) -> Result<bool, String> {
    let (url, token) = {
        let config = state.config.lock().unwrap();
        let url = config.ha_url.clone().ok_or("未配置 Home Assistant 地址")?;
        let token = config.ha_token.clone().ok_or("未配置 Home Assistant Token")?;
        (url, token)
    };

    let body = serde_json::json!({
        "entity_id": entity_id
    });

    let client = &state.client;
    let response = client.post(format!("{}/api/services/{}/{}", url, domain, service))
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    if response.status().is_success() {
        Ok(true)
    } else {
        Err(format!("调用服务失败: {}", response.status()))
    }
}

#[tauri::command]
async fn shutdown_system() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let output = std::process::Command::new("shutdown")
        .args(["/s", "/t", "0"])
        .output()
        .map_err(|e| e.to_string())?;

    #[cfg(not(target_os = "windows"))]
    let output = std::process::Command::new("shutdown")
        .args(["-h", "now"])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
async fn restart_system() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let output = std::process::Command::new("shutdown")
        .args(["/r", "/t", "0"])
        .output()
        .map_err(|e| e.to_string())?;

    #[cfg(not(target_os = "windows"))]
    let output = std::process::Command::new("shutdown")
        .args(["-r", "now"])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            get_system_info,
            get_network_stats,
            toggle_device,
            get_services,
            add_service,
            update_service,
            delete_service,
            check_service,
            get_all_status,
            get_app_config,
            update_app_config,
            shutdown_system,
            restart_system,
            check_ha_connection,
            get_ha_entities,
            call_ha_service,
        ])
        .setup(|app| {
            let quit_i = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .icon(app.default_window_icon().unwrap().clone())
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                window.hide().unwrap();
                api.prevent_close();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
