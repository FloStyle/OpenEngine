# ADR-0002: OpenEngine as an AI-native, all-in-one game engine

---
id: "ADR-0002"
title: "OpenEngine as an AI-native, all-in-one game engine (assistant résident, pas de chatbot à côté)"
status: "Proposed"
date: "2026-09-04"
phase: "Vision produit — fonde toutes les décisions d'architecture futures"
---

## Context

OpenEngine must be a serious Rust game engine (Vulkan, AAA rendering, ~140 fps
on complex scenes, large scenes) **and** an environment where an AI model
develops it from the inside. Two temptations must be rejected:

- **"Engine + chatbot à côté"** : a separate agent that reads random logs, needs
  deep knowledge of the engine internals, and fiddles with files through a
  generic protocol (e.g. MCP). Fragile, shallow coupling, high context cost.
- **"Embed a full agent runtime in the core"** : duplicating an LLM harness
  (multi-provider, agent loop, tool registry) inside a deterministic, `no_std`
  core. Couples model/keys/deps into the pure core, bloats it, and breaks the
  determinism wall.

The user wants a single **product** where the AI is a first-class, resident
operator that assists directly (editor copilot), and can offload the bulk of the
Rust code / architecture / debugging / feature work.

## Requirements (from the vision)

1. **Easy AI-assisted development** : the model works on a clean, typed,
   *semantic* surface (entities, components, systems-as-pure-logic, deltas) —
   **not** on Vulkan internals, not on "random logs".
2. **Deterministic** gameplay; **bugs caught at compile time** (strong typed
   contracts, `no_std` purity, ABI versioning) rather than runtime heisenbugs.
3. **Performant on large scenes** in Rust on Vulkan, with modern/AAA rendering.
4. **Unreal-like workflow but Rust-native** : scene/actor/component editing,
   logic authored as reloadable systems; the human keeps a familiar pipeline.
5. **Tomorrow, plug in any model** — an API key (DeepSeek/Anthropic/…) or a
   local `llama.cpp`/unsloth endpoint — and get a game-dev assistant. **No MCP,
   no hacky setup**, no requirement that the model understand C++/engine
   internals. The model should not need to read raw logs to debug.

## Decision: the AI is a resident operator on one observe → propose → verify → apply loop

The engine is layered so that the AI is **an operator**, identical in kind to the
human editor and the wasm game logic:

```
┌──────────────────────────  OpenEngine (produit)  ────────────────────────────┐
│                                                                              │
│  CORE (pur, no_std, déterministe — SANS LLM)                                 │
│    ECS SoA · composants typés/versionnés · logique = modules wasm reloadables│
│    fixed-point · [PURE] · compilation = filet de bugs                        │
│                                                                              │
│  HOST (Domain A, std)                                                        │
│    éditeur (Unreal-like) · rendu Vulkan moderne                              │
│    VÉRIFICATION-as-a-service (build/test/purity/déterminisme, typée)         │
│    apply réversible (delta/snapshot/rollback) · reload de module             │
│                                                                              │
│  COUCHE IA RÉSIDENTE (Domain A, std, optionnelle, montée à la demande)       │
│    · adaptateur modèle uniforme: n'importe quel endpoint (clé API ou         │
│      llama.cpp/unsloth local) — même contrat                                  │
│    · Assistant de dev jeu: observe l'état natif + le code source             │
│    · PROPOSE en langage du moteur: WorldDelta (features) + CodeDelta (Rust)  │
│    · VÉRIFIE via le moteur (erreurs typées, pas des logs)                    │
│    · APPLIQUE si vert · ROLLBACK sinon · l'humain approuve les breaking      │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Principles that keep the product "all-in-one" (not engine + chatbot)

1. **L'IA parle le langage du moteur.** observe = état + delta typés ; propose =
   deltas et patches ; jamais besoin de fouiller le pipeline Vulkan interne.
2. **Hiérarchie de confiance.** IA **propose** (faillible) ; le **moteur
   vérifie** (determinism/purity/tests/compilation) ; l'**humain approuve** les
   changements breaking (ABI). La machine a le dernier mot sur l'intégrité.
3. **Erreurs typées, pas de logs.** La verification renvoie au modèle des
   erreurs structurées (échec de build/test/purity/déterminisme + localisation),
   pas un dump de logs → le modèle corrige sans "connaissance profonde".
4. **Couche IA résidente ≠ agent runtime générique.** Elle embarque seulement :
   l'adaptateur modèle, un lecteur d'état+code, le générateur de propose, et
   l'appel à la gate du moteur. Pas de re-implantation d'un loop d'agent.
5. **Réside en Domain A**, jamais dans le core pur → le moteur reste déterministe
   même si le LLM plante ou est hors-ligne.

### Debug "sans logs random"

Le fichier de débogage de l'IA est le **delta + le hash + le gate de
déterminisme** : reproduire un bug = `observe` → rejouer une séquence → comparer
`hash` → le moteur localise où la dérive apparaît. Compilation et purity
attrapent le reste à la source.

## What this means for current foundations (no wasted work)

The seams needed for the resident AI later are **the same** ones useful today for
external-agent development:

| Seam (do now) | Use today (external harness) | Use later (resident AI) |
|---|---|---|
| Headless live core (`crates/harness`) | agent observe/mutate/verify | AI observe native state |
| `/prove` + `/transaction` (+ rollback) | safe external iteration | AI propose → moteur vérifie → rollback |
| Vérification typée as-a-service | verdict pour l'agent | erreurs typées pour l'IA |
| Logique reloadable (wasm) | boucle code↔état | feature/Réparabilité runtime |
| Modèle d'opérateur (trait `Operator`) | — | humain/jeu/IA = même pipeline |

## Consequences / Non-goals now

- **Now** : dev fondationnel peut rester sur un harnais externe (DeepSeek-Harness).
- **Do NOT now** : implémenter un agent-loop complet, un client LLM dans le core,
  ni un "self-heal controller". On pose seulement les seams + l'interface
  `Operator`/adaptateur.
- The pure core stays LLM-free and deterministic; the AI layer is a host-side,
  optional, swappable capability.
