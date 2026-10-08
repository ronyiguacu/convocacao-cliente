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

/// Windows: grava o inicio automatico (Run, com aspas e --autostart, pra saber que subiu sozinho) e,
/// desde a 1.1.1, religa se ele foi desligado nos "Aplicativos de inicializacao" (Gerenciador de
/// Tarefas ou Configuracoes): o Windows guarda isso em StartupApproved\Run, um valor binario cujo
/// 1o byte impar quer dizer desligado (Rony, 08/10/2026: a Convocacao da Cinthia nao subia com o
/// computador). Devolve como estava: ok, religado, desligado (nao deu pra religar) ou sem_registro.
#[cfg(windows)]
fn habilitar_autostart() -> String {
    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    use winreg::{RegKey, RegValue};
    let Ok(exe) = std::env::current_exe() else {
        return "sem_registro".into();
    };
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok((run, _)) = hkcu.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") else {
        return "sem_registro".into();
    };
    // Aspas obrigatorias por causa do espaco no caminho do usuario.
    let valor = format!("\"{}\" --autostart", exe.display());
    if run.set_value("Convocacao", &valor).is_err() {
        return "sem_registro".into();
    }
    let mut estado = "ok";
    if let Ok(aprovados) = hkcu.open_subkey_with_flags(
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run",
        KEY_READ | KEY_SET_VALUE,
    ) {
        if let Ok(atual) = aprovados.get_raw_value("Convocacao") {
            if atual.bytes.first().map(|b| b & 1 == 1).unwrap_or(false) {
                let mut bytes = vec![0u8; 12];
                bytes[0] = 2;
                let ligado = RegValue {
                    bytes,
                    vtype: RegType::REG_BINARY,
                };
                estado = if aprovados.set_raw_value("Convocacao", &ligado).is_ok() {
                    "religado"
                } else {
                    "desligado"
                };
            }
        }
    }
    estado.into()
}

/// macOS: grava um LaunchAgent — o app passa a iniciar junto com o sistema
/// (antes era uma funcao vazia: no Mac o app simplesmente nao subia sozinho).
#[cfg(target_os = "macos")]
fn habilitar_autostart() -> String {
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
    <key>ProgramArguments</key><array><string>{}</string><string>--autostart</string></array>
    <key>RunAtLoad</key><true/>
</dict>
</plist>
"#,
                exe.display()
            );
            if fs::write(dir.join("com.iguacu.convocacao.plist"), plist).is_ok() {
                return "ok".into();
            }
        }
    }
    "sem_registro".into()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn habilitar_autostart() -> String {
    "nao_se_aplica".into()
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
    overlays: Mutex<Vec<String>>, // labels das janelas de alerta abertas
    dados_alerta: Mutex<Option<serde_json::Value>>, // dados da chamada atual
    recados: Mutex<Option<serde_json::Value>>, // mensagens do chat na lateral (1.1.0)
    inicio: Mutex<Option<serde_json::Value>>, // como o app subiu: versao, sozinho, autostart (1.1.1)
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

    let _ = habilitar_autostart();

    // Reinicia: agora ja configurado, sobe conectado.
    app.restart()
}

/// Mostra o aviso em tela cheia em TODOS os monitores (janelas ja existem,
/// so aparecem e sao reposicionadas na tela correta).
///
/// Assincrono de proposito (1.1.1): comando sincrono roda na thread principal DENTRO do aviso do
/// WebView2 que trouxe o pedido, e o run_on_main_thread ali executa na hora. Se nesse momento
/// precisar CRIAR janela (monitor ligado depois que o app abriu: garantir_overlays), o Windows
/// trava o app inteiro (o WebView2 novo espera um aviso que nunca chega). Assincrono, o pedido
/// volta na hora e o run_on_main_thread roda depois, no laco principal, onde criar janela e seguro.
#[tauri::command]
async fn mostrar_alerta(app: AppHandle, id: String, origem: String, motivo: String) {
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
///
/// Assincrono de proposito (1.1.1): na 1.1.0 era sincrono e, no Windows, criar a janela "recados"
/// dentro do aviso do WebView2 travava o app inteiro na primeira mensagem (Laura e Junior, 08/10:
/// o app seguia dando sinal de ligado, mas nao mostrava mais nada, nem convocacao). Ver mostrar_alerta.
#[tauri::command]
async fn mostrar_recados(app: AppHandle, lista: serde_json::Value) {
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

/// Como o app subiu (versao, se foi sozinho com o computador, o iniciar com o sistema): a janela
/// oculta conta pro banco e o Diagnostico mostra.
#[tauri::command]
fn inicio_info(app: AppHandle) -> Option<serde_json::Value> {
    app.state::<AppState>().inicio.lock().unwrap().clone()
}

fn overlay_labels(app: &AppHandle) -> Vec<String> {
    app.state::<AppState>().overlays.lock().unwrap().clone()
}

// Atualizacao: ao abrir e, desde a 1.1.1, de 30 em 30 min com o app ligado. Antes era so ao abrir,
// e a versao nova so chegava quando a pessoa reiniciava o computador (Rony, 08/10/2026: a 1.1.0 saiu
// e nenhum computador pegou no mesmo dia). Nunca com um alerta de convocacao na tela; se chegar um
// no meio da troca, o app reabre e mostra de novo (a chamada continua pendente no banco).
const ATUALIZAR_A_CADA: std::time::Duration = std::time::Duration::from_secs(30 * 60);

fn alerta_na_tela(app: &AppHandle) -> bool {
    app.state::<AppState>()
        .dados_alerta
        .lock()
        .map(|d| d.is_some())
        .unwrap_or(true)
}

/// Procura versao nova e, tendo, baixa e instala. Devolve true se instalou (falta reiniciar).
async fn atualizar_se_tiver(app: &AppHandle) -> bool {
    if alerta_na_tela(app) {
        return false;
    }
    let Ok(updater) = app.updater() else {
        return false;
    };
    let Ok(Some(update)) = updater.check().await else {
        return false;
    };
    if alerta_na_tela(app) {
        return false;
    }
    update.download_and_install(|_, _| {}, || {}).await.is_ok()
}

fn checar_atualizacao(app: AppHandle) {
    let ao_abrir = app.clone();
    tauri::async_runtime::spawn(async move {
        if atualizar_se_tiver(&ao_abrir).await {
            ao_abrir.restart();
        }
    });
    std::thread::spawn(move || loop {
        std::thread::sleep(ATUALIZAR_A_CADA);
        let agora = app.clone();
        let instalou = tauri::async_runtime::block_on(async move { atualizar_se_tiver(&agora).await });
        if instalou {
            app.restart();
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
            abrir_no_cosmo,
            inicio_info
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
                // Ja configurado: garante o inicio automatico (com aspas, religando se foi desligado)
                // e anota como o app subiu, pra contar pro banco (convocacao_app_inicio).
                let autostart = habilitar_autostart();
                let sozinho = std::env::args().any(|a| a == "--autostart");
                *app.state::<AppState>().inicio.lock().unwrap() = Some(serde_json::json!({
                    "versao": app.package_info().version.to_string(),
                    "sozinho": sozinho,
                    "autostart": autostart,
                }));

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
