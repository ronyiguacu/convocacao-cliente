# Guia do novo Cliente de Convocação (Tauri)

Este é o app que roda no PC de cada funcionário. Ele foi refeito em **Tauri**
(igual ao seu *pulso*), então é leve e **não trava** na hora da convocação.
O servidor no Render **continua o mesmo** — não precisa mexer nele.

## O que ficou melhor

- Fica conectado o tempo todo usando quase nada de memória.
- Quando chega um chamado, abre um **aviso em tela cheia em todos os monitores**,
  com som e piscando, sem poder fechar até a pessoa clicar em **CONFIRMAR**.
- Inicia sozinho junto com o Windows e fica no ícone ao lado do relógio.
- **Atualiza sozinho**: quando você publica uma versão nova, todos os PCs se
  atualizam automaticamente, sem você reinstalar nada.

---

## Parte 1 — Publicar (o jeito fácil: o GitHub compila pra você)

Você **não precisa** compilar nada no seu PC. O GitHub faz isso na nuvem.

1. **Crie um repositório no GitHub** (pode ser privado) e suba esta pasta
   `cliente-tauri` inteira pra lá.

2. **Ajuste o endereço do seu repositório** no arquivo
   `src-tauri/tauri.conf.json`. Troque, dentro de `endpoints`:
   `SEU_USUARIO/SEU_REPOSITORIO` pelo seu usuário e nome do repositório.
   Exemplo: `https://github.com/rony/convocacao/releases/latest/download/latest.json`

3. **Cadastre a chave de assinatura** (uma única vez). No GitHub, vá em
   `Settings > Secrets and variables > Actions > New repository secret` e crie:
   - `TAURI_SIGNING_PRIVATE_KEY` → o conteúdo do arquivo
     `CHAVE-PRIVADA-NAO-SUBIR.txt` (a linha grande indicada lá).
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` → deixe **vazio**.

   (A chave pública já está dentro do projeto. Não suba o arquivo da chave
   privada para o GitHub — o `.gitignore` já evita isso.)

4. **Gere a primeira versão.** No seu PC, dentro da pasta do projeto, rode:

   ```
   git tag v1.0.0
   git push origin v1.0.0
   ```

   Isso dispara o GitHub, que **compila o instalador sozinho** e cria uma
   "Release" com o programa pronto pra baixar. Acompanhe em **Actions** no site
   do GitHub (uns 5–10 minutos).

---

## Parte 2 — Instalar no PC do funcionário (uma vez só)

1. Na página de **Releases** do seu repositório, baixe o instalador
   (`Convocacao_x.x.x_x64-setup.exe`).
2. Rode ele no PC do funcionário.
3. Na primeira vez, ele pede **nome** e **setor** (igual antes). Depois disso
   nunca mais pergunta, inicia sozinho com o Windows e fica no canto perto do
   relógio.

Pronto. Daqui pra frente esse PC se atualiza sozinho.

---

## Parte 3 — Lançar uma atualização depois

Sempre que quiser mudar algo:

1. Edite o que precisar.
2. Suba o **número da versão** em **dois** lugares (use o mesmo número):
   - `package.json` → `"version"`
   - `src-tauri/tauri.conf.json` → `"version"`
   - `src-tauri/Cargo.toml` → `version`
   (ex.: de `1.0.0` para `1.0.1`)
3. Crie a tag nova e suba:

   ```
   git tag v1.0.1
   git push origin v1.0.1
   ```

O GitHub compila e publica. **Todos os PCs pegam a atualização sozinhos** na
próxima vez que abrirem (ou reiniciarem). Você não toca em nenhum PC.

---

## (Opcional) Testar no seu PC antes de publicar

Só se você quiser ver rodando localmente. Precisa ter **Node** e **Rust**
instalados (você já instalou na época do *pulso*). Na pasta do projeto:

```
npm install
npm run dev
```

Para gerar um instalador local: `npm run build`.

---

## Observações importantes

- **O servidor não muda.** O app fala exatamente os mesmos "eventos" do app
  antigo (`identificar`, `chamada`, `confirmado`) e busca os setores em
  `/setores`. É 100% compatível com o que já está no Render.
- Se a lista de **setores** não carregar no cadastro (cair pra "Outro"), é só o
  servidor não estar liberando acesso externo (CORS) para esse endereço — me
  avise que ajusto. Não atrapalha o funcionamento do chamado.
- O endereço do servidor está no topo dos arquivos `src/index.html` e
  `src/cadastro.html` (`https://painel-servidor.onrender.com`). Se um dia mudar
  o servidor, troque nesses dois lugares.
- **Guarde o arquivo da chave privada.** Se perder, as atualizações automáticas
  param de funcionar (aí teria que reinstalar nos PCs com uma chave nova).
