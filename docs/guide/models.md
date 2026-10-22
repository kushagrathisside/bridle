# Models and runtimes

This page lists the model weights and inference runtimes for each model slot, with sources, checksums and
licences, and the resource use measured on the release 1 target. It is for an engineer downloading and verifying
the files. Weights are never bundled or committed (decision D-2 and R-42.1): download them yourself into
`D:\models`.

> **Status:** Release 1. Slot `main` is in use and measured on the target machine. Slots `micro` (M4) and `deep`
> (M5) are planned. Each file below was downloaded and its SHA-256 checked on 2026-10-21.

## Weights

| Slot | Model | Quantisation | Source (revision) | File | SHA-256 | Licence |
|---|---|---|---|---|---|---|
| `main` | Qwen3.5-9B | Q4_K_M, 5.68 GB | `unsloth/Qwen3.5-9B-GGUF` @ `3885219b` | `Qwen3.5-9B-Q4_K_M.gguf` | `03b74727…52b7e8` | Apache-2.0 |
| `micro` (M4) | LFM2.5-1.2B-Thinking | Q4_K_M, 0.73 GB | `LiquidAI/LFM2.5-1.2B-Thinking-GGUF` @ `9584426e` | `LFM2.5-1.2B-Thinking-Q4_K_M.gguf` | `7223a220…12e1231b` | LFM Open License v1.0 (research use; see licence notes) |
| `deep` (M5) | Ternary Bonsai 2 27B | PTQ1_0, 5.95 GB | `prism-ml/Ternary-Bonsai-2-27B-gguf` @ `b072e1d3` | `Ternary-Bonsai-2-27B-PTQ1_0.gguf` | `53107f53…fe33ee3` | Apache-2.0 |

The official `Qwen/Qwen3.5-9B-GGUF` repository did not respond (HTTP 401). The unsloth GGUF is the one used.

## Runtimes

| Name | Build | Binary | Used for |
|---|---|---|---|
| llama.cpp | b11756, CUDA 12.4, MIT | `llama-server.exe` | `main`, `micro` |
| PrismML llama.cpp fork | b10787 (`73bfe6c`), CUDA 12.4, MIT | `llama-server.exe` | `deep` (PTQ1_0 only loads here) |

Upstream b11756 does not know the PTQ1_0 tensor type (143), so the fork is required for Bonsai.

## Measured resource use (`main` slot)

Measured on the release 1 target with Qwen3.5-9B:

| Measure | Value |
|---|---|
| Health ready after start | about 11 s |
| VRAM with Qwen loaded | 5443 MiB of 8188 MiB |
| Server process memory | about 3.5 GB |
| Host free RAM with the server running | 382 MB of 16 GB (the WSL VM also held about 3.5 GB) |
| Generation speed (server log) | about 42 tokens/s |

The low free RAM is why the prompt cache is capped at 1 GB and `parallel` is 1 (see
[configuration.md](configuration.md#keys-you-are-most-likely-to-change)).

## Estimates

Estimated from the GGUF headers, not measured under load:

| Configuration | VRAM |
|---|---|
| Qwen at 16K context | about 5.9 GiB |
| Bonsai at 16K context | about 6.7 GiB |
| Both together | about 12.9 GiB |

## Speculative decoding (MTP)

Neither local GGUF contains an MTP head, so MTP is not used. `agent health` reports "speculative decoding active"
only for a real draft type, and reports off for `"none"`.

## Limits

- Qwen and Bonsai cannot run at the same time on 8 GB of VRAM.
- Bonsai at 24K context with an f16 KV cache would not fit.
- The `micro` model's licence (LFM Open License v1.0) is for research use.

## Related

- [installation.md](installation.md): where to place the weights and runtimes.
- [configuration.md](configuration.md): model slot keys such as `context`, `parallel` and `cache_ram_mib`.
- [troubleshooting.md](troubleshooting.md): model start-up and health problems.
- [../reference/modules/runtime-serving.md](../reference/modules/runtime-serving.md): reference for launching and stopping model servers.
- [../decisions/0002-runtime-os-repo-location-and-deviation.md](../decisions/0002-runtime-os-repo-location-and-deviation.md): runtime OS decision.
