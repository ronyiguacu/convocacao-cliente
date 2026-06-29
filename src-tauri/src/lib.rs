use std::fs;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_updater::UpdaterExt as _;

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
    let p = caminho_config(app);
    let texto = fs::read_to_string(p).ok()?;
    serde_json::from_str::<Config>(&texto).ok()
}

// ---------- Estado em memoria ----------

#[derive(Default)]
struct AppState {
    // Dados da chamada atual, para a janela de alerta buscar quando carregar.
    alerta: Mutex<Option<serde_json::Value>>,
    // Labels das janelas de alerta abertas no momento.
    overlays: Mutex<Vec<String>>,
}

// ---------- Comandos chamados pelo frontend ----------

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

    // Avisa a janela oculta para conectar ao servidor.
    let _ = app.emit("config-salva", cfg.clone());

    // Atualiza o texto do menu da bandeja.
    let _ = atualizar_tray(&app, Some(cfg.nome.clone()));

    // Fecha a janela de cadastro.
    if let Some(w) = app.get_webview_window("cadastro") {
        let _ = w.destroy();
    }
    Ok(())
}

#[tauri::command]
fn abrir_cadastro(app: AppHandle) {
    if app.get_webview_window("cadastro").is_some() {
        return;
    }
    let _ = WebviewWindowBuilder::new(&app, "cadastro", WebviewUrl::App("cadastro.html".into()))
        .title("Configuracao inicial")
        .inner_size(380.0, 340.0)
        .resizable(false)
        .center()
        .build();
}

/// Chamado pela janela oculta quando o servidor manda uma "chamada".
/// Abre um aviso em tela cheia em TODOS os monitores.
#[tauri::command]
fn mostrar_alerta(app: AppHandle, id: String, origem: String, motivo: String) {
    fechar_overlays(&app);

    // Guarda os dados para a janela de alerta buscar ao carregar.
    {
        let state = app.state::<AppState>();
        *state.alerta.lock().unwrap() =
            Some(serde_json::json!({ "id": id, "origem": origem, "motivo": motivo }));
    }

    // Descobre os monitores a partir de qualquer janela existente.
    let base = app
        .get_webview_window("oculta")
        .or_else(|| app.webview_windows().values().next().cloned());

    let monitores = base
        .as_ref()
        .and_then(|w| w.available_monitors().ok())
        .unwrap_or_default();

    if monitores.is_empty() {
        criar_overlay(&app, 0, 0.0, 0.0, 1280.0, 720.0);
    } else {
        for (i, m) in monitores.iter().enumerate() {
            let escala = m.scale_factor();
            let pos = m.position();
            let tam = m.size();
            let x = pos.x as f64 / escala;
            let y = pos.y as f64 / escala;
            let w = tam.width as f64 / escala;
            let h = tam.height as f64 / escala;
            criar_overlay(&app, i, x, y, w, h);
        }
    }
}

#[tauri::command]
fn pegar_dados_alerta(app: AppHandle) -> Option<serde_json::Value> {
    let state = app.state::<AppState>();
    let dados = state.alerta.lock().unwrap().clone();
    dados
}

/// Chamado pela janela de alerta quando o funcionario clica em CONFIRMAR.
#[tauri::command]
fn confirmar(app: AppHandle, id: String) {
    // Pede para a janela oculta avisar o servidor.
    let _ = app.emit("confirmar-chamada", serde_json::json!({ "id": id }));
    // Fecha todos os avisos.
    fechar_overlays(&app);
    let state = app.state::<AppState>();
    *state.alerta.lock().unwrap() = None;
}

// ---------- Auxiliares ----------

fn criar_overlay(app: &AppHandle, idx: usize, x: f64, y: f64, w: f64, h: f64) {
    let label = format!("alerta-{idx}");
    let resultado =
        WebviewWindowBuilder::new(app, &label, WebviewUrl::App("alerta.html".into()))
            .title("Voce foi chamado!")
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(false)
            .resizable(false)
            .closable(false)
            .minimizable(false)
            .maximizable(false)
            .position(x, y)
            .inner_size(w, h)
            .focused(true)
            .build();

    if resultado.is_ok() {
        let state = app.state::<AppState>();
        state.overlays.lock().unwrap().push(label);
    }
}

fn fechar_overlays(app: &AppHandle) {
    let state = app.state::<AppState>();
    let labels: Vec<String> = state.overlays.lock().unwrap().drain(..).collect();
    for label in labels {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.destroy();
        }
    }
}

fn atualizar_tray(app: &AppHandle, nome: Option<String>) -> tauri::Result<()> {
    let rotulo = match nome {
        Some(n) => format!("Logado como: {n}"),
        None => "Nao configurado".to_string(),
    };
    let item_nome = MenuItemBuilder::with_id("nome", rotulo)
        .enabled(false)
        .build(app)?;
    let item_sair = MenuItemBuilder::with_id("sair", "Sair").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&item_nome)
        .separator()
        .item(&item_sair)
        .build()?;

    if let Some(tray) = app.tray_by_id("principal") {
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

fn checar_atualizacao(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Ok(updater) = app.updater() {
            if let Ok(Some(update)) = updater.check().await {
                let _ = update
                    .download_and_install(|_, _| {}, || {})
                    .await;
                app.restart();
            }
        }
    });
}

// ---------- Ponto de entrada ----------

pub fn run() {
    tauri::Builder::default()
        // Garante que so um app rode por vez.
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
            ler_config,
            salvar_config,
            abrir_cadastro,
            mostrar_alerta,
            pegar_dados_alerta,
            confirmar
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Bandeja do sistema.
            let nome_inicial = ler_config_arquivo(&handle).map(|c| c.nome);
            let item_nome = MenuItemBuilder::with_id(
                "nome",
                match &nome_inicial {
                    Some(n) => format!("Logado como: {n}"),
                    None => "Nao configurado".to_string(),
                },
            )
            .enabled(false)
            .build(app)?;
            let item_sair = MenuItemBuilder::with_id("sair", "Sair").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&item_nome)
                .separator()
                .item(&item_sair)
                .build()?;

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

            // Liga inicio automatico se ja estiver configurado.
            if nome_inicial.is_some() {
                let _ = app.autolaunch().enable();
            }

            // Janela oculta que mantem a conexao com o servidor.
            WebviewWindowBuilder::new(&handle, "oculta", WebviewUrl::App("index.html".into()))
                .title("convocacao")
                .visible(false)
                .skip_taskbar(true)
                .build()?;

            // Verifica atualizacao em segundo plano.
            checar_atualizacao(handle.clone());

            Ok(())
        })
        .on_window_event(|window, event| {
            // Impede que fechar a janela oculta encerre o app.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "oculta" {
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o aplicativo de convocacao");
}
