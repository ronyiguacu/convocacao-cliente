use std::fs;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_updater::UpdaterExt as _;

// WebView2 (Windows) sem QUIC/HTTP3 — 1.0.10. Rony, 06/10/2026: num computador o app do Cosmo
// parou em ERR_QUIC_PROTOCOL_ERROR. O Supabase e o Cloudflare oferecem HTTP/3 (QUIC, UDP 443),
// e antivírus, firewall ou a rede de alguns computadores atrapalham esse protocolo: a busca de
// chamadas falharia calada. Com isto tudo vai por HTTPS comum. Os três primeiros são o padrão do
// wry, que este argumento substitui. TODA janela tem que usar ARGS_WEBVIEW2: janelas do mesmo
// app com argumentos diferentes não abrem (o WebView2 recusa) — e a do alerta é uma delas.
const ARGS_WEBVIEW2: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-quic";

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

/// macOS: grava um LaunchAgent — o app passa a iniciar junto com o sistema
/// (antes era uma funcao vazia: no Mac o app simplesmente nao subia sozinho).
#[cfg(target_os = "macos")]
fn habilitar_autostart() {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(home) = std::env::var_os("HOME") {
            let dir = std::path::Path::new(&home).join("Library/LaunchAgents");
            let _ = fs::create_dir_all(&dir);
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.iguacu.convocacao</string>
    <key>ProgramArguments</key><array><string>{}</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
                exe.display()
            );
            let _ = fs::write(dir.join("com.iguacu.convocacao.plist"), plist);
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
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
    overlays: Mutex<Vec<String>>, // labels das janelas de alerta abertas
    dados_alerta: Mutex<Option<serde_json::Value>>, // dados da chamada atual
    recados: Mutex<Option<serde_json::Value>>, // mensagens do chat na lateral (1.1.0)
}

// ---------- Comandos chamados pelo frontend ----------

/// Cada janela carrega o mesmo index.html e pergunta "qual e o meu papel?".
#[tauri::command]
fn qual_view(window: WebviewWindow) -> String {
    let l = window.label();
    if l == "cadastro" {
        "cadastro".to_string()
    } else if l == "diagnostico" {
        "diagnostico".to_string()
    } else if l.starts_with("alerta") {
        "alerta".to_string()
    } else if l == "recados" {
        "recados".to_string()
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
    acender_tela();
    let payload = serde_json::json!({ "id": id, "origem": origem, "motivo": motivo });
    *app.state::<AppState>().dados_alerta.lock().unwrap() = Some(payload.clone());

    // Tudo na thread principal (obrigatorio no macOS pra criar/mexer em janela).
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        // Re-enumera os monitores AGORA — resolve notebook que plugou/desplugou
        // monitor depois que o app iniciou (antes as janelas eram criadas so no boot).
        garantir_overlays(&app2);
        for label in overlay_labels(&app2) {
            if let Some(w) = app2.get_webview_window(&label) {
                let _ = w.set_visible_on_all_workspaces(true);
                let _ = w.show();
                let _ = w.set_always_on_top(true);
                let _ = w.set_focus();
            }
        }
        let _ = app2.emit("disparar-alerta", payload);
    });
}

/// Cria OU reposiciona uma janela de alerta por monitor, conforme os monitores
/// que existem NESTE momento (chamada no inicio e a cada alerta). Coordenadas
/// FISICAS (mais confiavel entre monitores).
fn garantir_overlays(app: &AppHandle) {
    let base = app
        .get_webview_window("oculta")
        .or_else(|| app.get_webview_window("cadastro"));
    let monitores = base
        .as_ref()
        .and_then(|w| w.available_monitors().ok())
        .unwrap_or_default();

    let mut labels = Vec::new();
    for (i, m) in monitores.iter().enumerate() {
        let pos = *m.position(); // PhysicalPosition<i32>
        let tam = *m.size(); // PhysicalSize<u32>
        let label = format!("alerta-{i}");

        // Ja existe? So reposiciona nas coordenadas atuais do monitor.
        if let Some(win) = app.get_webview_window(&label) {
            let _ = win.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
            let _ = win.set_size(tauri::PhysicalSize::new(tam.width, tam.height));
            let _ = win.set_visible_on_all_workspaces(true);
            labels.push(label);
            continue;
        }

        let build = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .additional_browser_args(ARGS_WEBVIEW2)
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
            let _ = win.set_position(tauri::PhysicalPosition::new(pos.x, pos.y));
            let _ = win.set_size(tauri::PhysicalSize::new(tam.width, tam.height));
            let _ = win.set_visible_on_all_workspaces(true);
            labels.push(label);
        }
    }

    // Janelas de monitores que nao existem mais: esconde (evita "alerta fantasma").
    let anteriores = app.state::<AppState>().overlays.lock().unwrap().clone();
    for velho in anteriores {
        if !labels.contains(&velho) {
            if let Some(w) = app.get_webview_window(&velho) {
                let _ = w.hide();
            }
        }
    }

    *app.state::<AppState>().overlays.lock().unwrap() = labels;
}

/// Batimento do lado nativo: a cada 20s emite um evento pro JS buscar pendencias.
/// O JS suspenso pelo App Nap (macOS) acorda quando recebe evento do Rust —
/// e uma thread nativa nao e congelada como os timers do webview.
fn iniciar_batimento(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let _ = app.emit("verificar-pendentes", ());
    });
}

// ---------- Mac acordado ----------
//
// Mac em repouso nao recebe convocacao na hora: a chamada so aparece quando ele acorda.

/// Roda o `caffeinate` do proprio macOS e recolhe o processo quando ele sair (sem zumbi).
#[cfg(target_os = "macos")]
fn caffeinate(args: &[&str]) {
    match std::process::Command::new("/usr/bin/caffeinate").args(args).spawn() {
        Ok(mut filho) => {
            std::thread::spawn(move || {
                let _ = filho.wait();
            });
        }
        Err(e) => eprintln!("caffeinate {args:?}: {e}"),
    }
}

/// Enquanto a Convocacao estiver aberta e o Mac estiver NA TOMADA, ele nao entra em repouso.
/// `-s` so vale no adaptador de energia (na bateria nao faz nada, entao nao drena notebook) e
/// a tela continua apagando normalmente. `-w <pid>` faz o caffeinate sair junto com o app: se
/// a Convocacao fechar ou travar, o Mac volta a dormir como sempre.
#[cfg(target_os = "macos")]
fn manter_mac_acordado() {
    let pid = std::process::id().to_string();
    caffeinate(&["-s", "-w", &pid]);
}

#[cfg(not(target_os = "macos"))]
fn manter_mac_acordado() {}

/// Chegou convocacao: acende a tela, se ela tinha apagado por inatividade (o alerta estaria
/// la, mas com o monitor desligado). `-u` e o mesmo que mexer no mouse, por 10 segundos.
#[cfg(target_os = "macos")]
fn acender_tela() {
    caffeinate(&["-u", "-t", "10"]);
}

#[cfg(not(target_os = "macos"))]
fn acender_tela() {}

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

// ---------- Chat da empresa: a mensagem subindo na lateral esquerda (1.1.0) ----------
//
// Rony, 07/10/2026: mensagem do Chat do Cosmo, com o Cosmo fora de vista, aparece "subindo na
// lateral esquerda e só sai da tela quando clicar no X de fechar ou responder" — não o alerta de
// tela cheia. A janela "oculta" pergunta ao banco (chat_para_o_app) e manda a lista pra cá; esta
// janela fica no canto de baixo à esquerda da tela principal, por cima das outras, sem roubar o foco
// de quem está digitando em outro programa (nasce sem foco e é fechada quando a lista esvazia —
// mostrar de novo uma janela escondida no Windows pegaria o foco). Responder e o X voltam pra
// "oculta", que fala com o banco.

const RECADOS_LARGURA: f64 = 380.0;
const RECADOS_MARGEM: f64 = 14.0;

/// Canto de baixo à esquerda da tela principal (fora da barra de tarefas), em pixels físicos.
fn posicionar_recados(w: &WebviewWindow, altura_logica: f64) {
    let monitor = match w.primary_monitor() {
        Ok(Some(m)) => m,
        _ => return,
    };
    let escala = monitor.scale_factor();
    let area = monitor.work_area();
    let margem = (RECADOS_MARGEM * escala).round() as i32;
    let largura = (RECADOS_LARGURA * escala).round() as u32;
    let maxima = (area.size.height as i32 - 2 * margem).max(120) as u32;
    let altura = ((altura_logica * escala).round() as u32).clamp(80, maxima);
    let x = area.position.x + margem;
    let y = area.position.y + area.size.height as i32 - altura as i32 - margem;
    let _ = w.set_size(tauri::PhysicalSize::new(largura, altura));
    let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
}

fn janela_recados(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(w) = app.get_webview_window("recados") {
        return Some(w);
    }
    let w = WebviewWindowBuilder::new(app, "recados", WebviewUrl::App("index.html".into()))
        .additional_browser_args(ARGS_WEBVIEW2)
        .title("Mensagens do Cosmo")
        .inner_size(RECADOS_LARGURA, 150.0)
        .visible(false)
        .focused(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .shadow(true)
        .build()
        .ok()?;
    posicionar_recados(&w, 150.0);
    let _ = w.set_visible_on_all_workspaces(true);
    let _ = w.show();
    Some(w)
}

/// A "oculta" manda a lista do banco: vazia fecha a janela; com algo, abre (se preciso) e entrega.
#[tauri::command]
fn mostrar_recados(app: AppHandle, lista: serde_json::Value) {
    let vazia = lista.as_array().map(|l| l.is_empty()).unwrap_or(true);
    *app.state::<AppState>().recados.lock().unwrap() = if vazia { None } else { Some(lista.clone()) };
    let app2 = app.clone();
    let _ = app.run_on_main_thread(move || {
        if vazia {
            if let Some(w) = app2.get_webview_window("recados") {
                let _ = w.close();
            }
            return;
        }
        if app2.get_webview_window("recados").is_some() {
            let _ = app2.emit_to("recados", "recados", lista);
        } else {
            janela_recados(&app2); // ela busca a lista ao carregar (pegar_recados)
        }
    });
}

/// A janela das mensagens busca a lista ao carregar.
#[tauri::command]
fn pegar_recados(app: AppHandle) -> Option<serde_json::Value> {
    app.state::<AppState>().recados.lock().unwrap().clone()
}

/// A janela das mensagens diz a altura do que tem dentro; ela cresce pra cima a partir do canto.
#[tauri::command]
fn recados_tamanho(window: WebviewWindow, altura: f64) {
    if window.label() != "recados" {
        return;
    }
    let w = window.clone();
    let _ = window.run_on_main_thread(move || posicionar_recados(&w, altura));
}

/// "Abrir no Cosmo": a conversa no navegador (o Cosmo abre nela).
#[tauri::command]
fn abrir_no_cosmo(conversa: String) -> Result<(), String> {
    let valido = conversa.len() == 36 && conversa.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if !valido {
        return Err("conversa invalida".into());
    }
    abrir_endereco(&format!("https://pulso-e8r.pages.dev/cosmo/?conversa={conversa}#chat"))
}

#[cfg(windows)]
fn abrir_endereco(url: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .creation_flags(0x0800_0000) // sem janela de console
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn abrir_endereco(url: &str) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn abrir_endereco(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.state::<AppState>().overlays.lock().unwrap().clone()
}

fn checar_atualizacao(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Ok(updater) = app.updater() {
            if let Ok(Some(update)) = updater.check().await {
                if update.download_and_install(|_, _| {}, || {}).await.is_ok() {
                    app.restart();
                }
            }
        }
    });
}

fn abrir_diagnostico(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("diagnostico") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }

    if let Ok(w) =
        WebviewWindowBuilder::new(app, "diagnostico", WebviewUrl::App("index.html".into()))
            .additional_browser_args(ARGS_WEBVIEW2)
            .title("Diagnostico Convocacao")
            .inner_size(560.0, 620.0)
            .resizable(true)
            .center()
            .build()
    {
        let _ = w.set_focus();
    }
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
            carregar_setores,
            mostrar_recados,
            pegar_recados,
            recados_tamanho,
            abrir_no_cosmo
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            let config = ler_config_arquivo(&handle);

            // Bandeja do sistema.
            let rotulo = match &config {
                Some(c) => format!("Logado como: {}", c.nome),
                None => "Nao configurado".to_string(),
            };
            let item_nome = MenuItemBuilder::with_id("nome", rotulo)
                .enabled(false)
                .build(app)?;
            let item_diag = MenuItemBuilder::with_id("diagnostico", "Diagnostico").build(app)?;
            let item_sair = MenuItemBuilder::with_id("sair", "Sair").build(app)?;
            let menu = MenuBuilder::new(app)
                .item(&item_nome)
                .separator()
                .item(&item_diag)
                .separator()
                .item(&item_sair)
                .build()?;
            TrayIconBuilder::with_id("principal")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Sistema de Convocacao")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "diagnostico" => abrir_diagnostico(app),
                    "sair" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if config.is_some() {
                // Ja configurado: garante o inicio automatico (com aspas) e sobe a conexao.
                habilitar_autostart();

                WebviewWindowBuilder::new(&handle, "oculta", WebviewUrl::App("index.html".into()))
                    .additional_browser_args(ARGS_WEBVIEW2)
                    .title("convocacao")
                    .visible(false)
                    .skip_taskbar(true)
                    .build()?;

                garantir_overlays(&handle);
                iniciar_batimento(handle.clone());
                manter_mac_acordado();
                checar_atualizacao(handle.clone());
            } else {
                // Primeira vez: abre a tela de cadastro (visivel).
                WebviewWindowBuilder::new(
                    &handle,
                    "cadastro",
                    WebviewUrl::App("index.html".into()),
                )
                .additional_browser_args(ARGS_WEBVIEW2)
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
