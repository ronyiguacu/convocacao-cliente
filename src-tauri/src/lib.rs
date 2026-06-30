use std::fs;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt as _;
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
    overlays: Mutex<Vec<String>>, // labels das janelas de alerta (pre-criadas)
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

    // Liga o inicio automatico com o Windows.
    let _ = app.autolaunch().enable();

    // Reinicia o app: agora ja configurado, ele sobe conectado e com os alertas prontos.
    app.restart()
}

/// Mostra o aviso em tela cheia em todos os monitores (janelas ja existem, so aparecem).
#[tauri::command]
fn mostrar_alerta(app: AppHandle, id: String, origem: String, motivo: String) {
    let payload = serde_json::json!({ "id": id, "origem": origem, "motivo": motivo });
    for label in overlay_labels(&app) {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
    let _ = app.emit("disparar-alerta", payload);
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
}

/// Retorna os labels das janelas de alerta (copia, sem segurar o lock).
fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.state::<AppState>().overlays.lock().unwrap().clone()
}

// ---------- Auxiliares ----------

fn criar_overlays(app: &AppHandle) {
    // Usa a janela oculta para descobrir os monitores.
    let base = app.get_webview_window("oculta");
    let monitores = base
        .as_ref()
        .and_then(|w| w.available_monitors().ok())
        .unwrap_or_default();

    let mut labels = Vec::new();
    for (i, m) in monitores.iter().enumerate() {
        let escala = m.scale_factor();
        let pos = m.position();
        let tam = m.size();
        let x = pos.x as f64 / escala;
        let y = pos.y as f64 / escala;
        let w = tam.width as f64 / escala;
        let h = tam.height as f64 / escala;
        let label = format!("alerta-{i}");

        let ok = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title("Voce foi chamado!")
            .visible(false)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .closable(false)
            .minimizable(false)
            .maximizable(false)
            .position(x, y)
            .inner_size(w, h)
            .build()
            .is_ok();

        if ok {
            labels.push(label);
        }
    }

    let state = app.state::<AppState>();
    *state.overlays.lock().unwrap() = labels;
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            qual_view,
            ler_config,
            salvar_config,
            mostrar_alerta,
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
                // Ja configurado: liga inicio automatico, sobe a janela oculta (conexao)
                // e deixa os avisos de tela cheia prontos (escondidos).
                let _ = app.autolaunch().enable();

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
                // A janela oculta nunca fecha de verdade.
                if window.label() == "oculta" {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o aplicativo de convocacao");
}
