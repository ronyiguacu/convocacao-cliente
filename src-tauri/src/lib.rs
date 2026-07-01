use std::fs;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_updater::UpdaterExt as _;

const SERVIDOR: &str = "https://painel-servidor.onrender.com";

/// Busca os setores no servidor pelo lado nativo (sem restricao de CORS do navegador).
#[tauri::command]
async fn carregar_setores() -> Vec<String> {
    let url = format!("{SERVIDOR}/setores");
    match reqwest::Client::new().get(&url).send().await {
        Ok(resp) => resp.json::<Vec<String>>().await.unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

// ---------- Inicio automatico (grava no registro com ASPAS) ----------

#[cfg(windows)]
fn habilitar_autostart() {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    if let Ok(exe) = std::env::current_exe() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((run, _)) =
            hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
        {
            // Aspas obrigatorias por causa do espaco no caminho do usuario.
            let valor = format!("\"{}\"", exe.display());
            let _ = run.set_value("Convocacao", &valor);
        }
    }
}

#[cfg(not(windows))]
fn habilitar_autostart() {}

// ---------- Configuracao do funcionario ----------

#[derive(Serialize, Deserialize, Clone)]
struct Config {
    nome: String,
    setor: String,
}

fn caminho_config(app: &AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_config_dir()
        .expect("nao foi possivel obter a pasta de configuracao");
    let _ = fs::create_dir_all(&dir);
    dir.join("config.json")
}

fn ler_config_arquivo(app: &AppHandle) -> Option<Config> {
    let texto = fs::read_to_string(caminho_config(app)).ok()?;
    serde_json::from_str::<Config>(&texto).ok()
}

// ---------- Estado em memoria ----------

#[derive(Default)]
struct AppState {
    overlays: Mutex<Vec<String>>,               // labels das janelas de alerta abertas
    dados_alerta: Mutex<Option<serde_json::Value>>, // dados da chamada atual
}

// ---------- Comandos chamados pelo frontend ----------

/// Cada janela carrega o mesmo index.html e pergunta "qual e o meu papel?".
#[tauri::command]
fn qual_view(window: WebviewWindow) -> String {
    let l = window.label();
    if l == "cadastro" {
        "cadastro".to_string()
    } else if l.starts_with("alerta") {
        "alerta".to_string()
    } else {
        "oculta".to_string()
    }
}

#[tauri::command]
fn ler_config(app: AppHandle) -> Option<Config> {
    ler_config_arquivo(&app)
}

#[tauri::command]
fn salvar_config(app: AppHandle, nome: String, setor: String) -> Result<(), String> {
    let cfg = Config { nome, setor };
    let texto = serde_json::to_string(&cfg).map_err(|e| e.to_string())?;
    fs::write(caminho_config(&app), texto).map_err(|e| e.to_string())?;

    habilitar_autostart();

    // Reinicia: agora ja configurado, sobe conectado.
    app.restart()
}

/// Mostra o aviso em tela cheia em TODOS os monitores (janelas ja existem,
/// so aparecem e sao reposicionadas na tela correta).
#[tauri::command]
fn mostrar_alerta(app: AppHandle, id: String, origem: String, motivo: String) {
    *app.state::<AppState>().dados_alerta.lock().unwrap() =
        Some(serde_json::json!({ "id": id, "origem": origem, "motivo": motivo }));

    for label in overlay_labels(&app) {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
    let payload = serde_json::json!({ "id": id, "origem": origem, "motivo": motivo });
    let _ = app.emit("disparar-alerta", payload);
}

/// Cria (uma vez, no inicio) uma janela de alerta por monitor, posicionada
/// em cada tela com coordenadas FISICAS (mais confiavel entre monitores).
fn criar_overlays(app: &AppHandle) {
    let base = app.get_webview_window("oculta");
    let monitores = base
        .as_ref()
        .and_then(|w| w.available_monitors().ok())
        .unwrap_or_default();

    let mut labels = Vec::new();
    for (i, m) in monitores.iter().enumerate() {
        let pos = *m.position(); // PhysicalPosition<i32>
        let tam = *m.size(); // PhysicalSize<u32>
        let label = format!("alerta-{i}");

        let build = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title("Voce foi chamado!")
            .visible(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .closable(false)
            .minimizable(false)
            .maximizable(false)
            .build();

        if let Ok(win) = build {
            // Posiciona/redimensiona em coordenadas fisicas exatas do monitor.
            let _ = win.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
            let _ = win.set_size(tauri::PhysicalSize::new(tam.width, tam.height));
            labels.push(label);
        }
    }
    *app.state::<AppState>().overlays.lock().unwrap() = labels;
}

/// A janela de alerta busca os dados ao carregar.
#[tauri::command]
fn pegar_dados_alerta(app: AppHandle) -> Option<serde_json::Value> {
    app.state::<AppState>().dados_alerta.lock().unwrap().clone()
}

/// O funcionario confirmou: avisa o servidor e esconde os avisos.
#[tauri::command]
fn confirmar(app: AppHandle, id: String) {
    let _ = app.emit("confirmar-chamada", serde_json::json!({ "id": id }));
    let _ = app.emit("parar-alerta", ());
    for label in overlay_labels(&app) {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.hide();
        }
    }
    *app.state::<AppState>().dados_alerta.lock().unwrap() = None;
}

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.state::<AppState>().overlays.lock().unwrap().clone()
}

fn checar_atualizacao(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Ok(updater) = app.updater() {
            if let Ok(Some(update)) = updater.check().await {
                let _ = update.download_and_install(|_, _| {}, || {}).await;
                app.restart();
            }
        }
    });
}

// ---------- Ponto de entrada ----------

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("cadastro") {
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            qual_view,
            ler_config,
            salvar_config,
            mostrar_alerta,
            pegar_dados_alerta,
            confirmar,
            carregar_setores
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let config = ler_config_arquivo(&handle);

            // Bandeja do sistema.
            let rotulo = match &config {
                Some(c) => format!("Logado como: {}", c.nome),
                None => "Nao configurado".to_string(),
            };
            let item_nome = MenuItemBuilder::with_id("nome", rotulo).enabled(false).build(app)?;
            let item_sair = MenuItemBuilder::with_id("sair", "Sair").build(app)?;
            let menu = MenuBuilder::new(app).item(&item_nome).separator().item(&item_sair).build()?;
            TrayIconBuilder::with_id("principal")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Sistema de Convocacao")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    if event.id().as_ref() == "sair" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            if config.is_some() {
                // Ja configurado: garante o inicio automatico (com aspas) e sobe a conexao.
                habilitar_autostart();

                WebviewWindowBuilder::new(&handle, "oculta", WebviewUrl::App("index.html".into()))
                    .title("convocacao")
                    .visible(false)
                    .skip_taskbar(true)
                    .build()?;

                criar_overlays(&handle);
                checar_atualizacao(handle.clone());
            } else {
                // Primeira vez: abre a tela de cadastro (visivel).
                WebviewWindowBuilder::new(&handle, "cadastro", WebviewUrl::App("index.html".into()))
                    .title("Configuracao inicial")
                    .inner_size(380.0, 340.0)
                    .resizable(false)
                    .center()
                    .build()?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "oculta" {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o aplicativo de convocacao");
}
