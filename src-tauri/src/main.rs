// Impede que uma janela de console preta apareca no Windows em modo release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    convocacao_cliente_lib::run();
}
