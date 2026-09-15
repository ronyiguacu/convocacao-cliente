# AGENTS.md — contrato de trabalho dos agentes neste repositório

> Este arquivo diz **como** se trabalha aqui. Claude (Cowork) e Codex seguem o mesmo contrato
> em todos os projetos do Rony — o que é específico deste repositório está na seção 7.
> Antes de qualquer tarefa: ler este arquivo e, se existirem, o `CLAUDE.md` e o `WORKLOG.md`.

## 1. Autonomia — fazer sem perguntar

Vale quando a ação é **reversível**, **verificável** e **não muda o que já está no ar**:

- Ler, buscar e medir: código, banco (`SELECT`), logs.
- Rodar teste, lint e build local.
- Escrever e editar código e documentação.
- Corrigir bug de escopo fechado, junto com o teste que prova a correção.
- Commitar e dar push na branch de trabalho.

## 2. Exige OK do Rony antes

Aqui a ação toca produção, permissão ou o que está instalado na máquina dos outros:

- **Escrita em banco de produção:** DDL, DML, migração aplicada, segredo, job agendado.
- **Permissão:** RLS, `GRANT`/`REVOKE`, papéis, policies, escopo de token.
- **Publicar versão:** tag `vX.Y.Z`, release, qualquer coisa que dispare atualização automática nas máquinas da equipe.
- **Apagar:** dados, objetos do banco, arquivos, branch remota.
- **Arquitetura:** trocar stack, dependência pesada ou padrão de dados.
- **Outro projeto ou outra conta:** nada neste repositório autoriza mexer em outro.

Ao pedir OK: diga **o plano, o impacto e como reverter**. Uma mensagem, não três.

## 3. Nunca

- Commitar segredo. O hook `pre-commit` barra, mas a regra vem antes do hook.
- Aplicar mudança direto em produção sem que ela vire arquivo versionado no repositório.
- Usar chave de servidor (`service_role`, token de admin) em qualquer coisa que o usuário final baixe.
- `git push --force` na branch principal.
- Trabalhar com a conta errada — ver seção 7.
- Marcar como pronto o que não foi verificado.

## 4. Fluxo padrão de uma tarefa

1. Ler este arquivo e, se existirem, `CLAUDE.md` e `WORKLOG.md`.
2. Fazer **uma fatia por vez**: a menor mudança que dá pra testar sozinha.
3. Verificar de verdade (seção 5).
4. Registrar: se o repositório tem `WORKLOG.md`, entrada nova no topo do histórico; se não tem, a **mensagem de commit** é o registro — ela precisa dizer o que mudou e por quê.
5. `git pull --rebase`, commit, push.

## 5. "Pronto" significa verificado

Antes de dizer que terminou, responder em uma linha cada:

- **Como foi testado?** Comando que rodou, tela que abriu, query que conferiu. "Deve funcionar" não conta.
- **Mexeu no que o usuário vê?** Então abriu e olhou.
- **Push feito?** Trabalho que não foi pro `origin` não existe pro outro agente.

## 6. Segredo

- Segredo de runtime vive em variável de ambiente ou cofre — nunca no código, nunca em migração, nunca em commit.
- No cliente, só chave pública/anon.
- Achou segredo no histórico? Trocar o arquivo não basta: **rotacionar o valor** e limpar o rastro.
- O hook em `.githooks/pre-commit` barra `.env`, token, JWT e chave privada antes do commit. Num clone novo, ativar com:

```
git config core.hooksPath .githooks
```

## 7. Este repositório

- **Projeto:** Cliente desktop da Convocacao (Tauri): alerta de tela cheia nos computadores da equipe, login com Conta Cosmo, Realtime do Supabase, bandeja, autostart e auto-update. Publicar = criar tag vX.Y.Z, o que dispara o build no GitHub Actions e atualiza TODAS as maquinas da equipe — so com OK do Rony.
- **GitHub:** `ronyiguacu/convocacao-cliente`
- **Conta fixada por repositório:** o usuário está embutido no remote e em `credential.username` no `.git/config` local, e o token de cada conta fica no Gerenciador de Credenciais do Windows. Por isso `push`, `pull` e `fetch` funcionam **qualquer que seja a conta ativa no `gh`** — não trocar conta pra commitar.
- **Exceção:** comandos do próprio `gh` (`gh pr`, `gh release`, `gh issue`) usam a conta ATIVA. Antes de usar `gh` aqui:

```
gh auth switch --hostname github.com --user ronyiguacu
```

- **Fronteira:** o projeto Meclico (conta `ronymarcosid`) é tocado por outro agente. Nada neste repositório autoriza mexer lá — e vice-versa.

---

*Contrato instalado em 2026-09-15. A versão de referência, com as particularidades de banco e migração, está no repositório `pulso` (Cosmo).*
