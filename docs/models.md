# Models and runtimes

Weights are never bundled or committed (decision D-2 and R-42.1). Download them yourself, into `D:\models`.
Each file below was downloaded and its SHA-256 checked on 2026-10-21.

## Weights

| Slot | Model | Quantisation | Source (revision) | File | SHA-256 | Licence |
|---|---|---|---|---|---|---|
| main | Qwen3.5-9B | Q4_K_M, 5.68 GB | `unsloth/Qwen3.5-9B-GGUF` @ `3885219b` | `Qwen3.5-9B-Q4_K_M.gguf` | `03b74727…52b7e8` | Apache-2.0 |
| micro (M4) | LFM2.5-1.2B-Thinking | Q4_K_M, 0.73 GB | `LiquidAI/LFM2.5-1.2B-Thinking-GGUF` @ `9584426e` | `LFM2.5-1.2B-Thinking-Q4_K_M.gguf` | `7223a220…12e1231b` | LFM Open License v1.0 (research use; see licence notes) |
| deep (M5) | Ternary Bonsai 2 27B | PTQ1_0, 5.95 GB | `prism-ml/Ternary-Bonsai-2-27B-gguf` @ `b072e1d3` | `Ternary-Bonsai-2-27B-PTQ1_0.gguf` | `53107f53…fe33ee3` | Apache-2.0 |

The official `Qwen/Qwen3.5-9B-GGUF` repository did not respond (HTTP 401). The unsloth GGUF is what was used.

## Runtimes

| Name | Build | Binary | Used for |
|---|---|---|---|
| llama.cpp | b11756, CUDA 12.4, MIT | `llama-server.exe` | main, micro |
| PrismML llama.cpp fork | b10787 (`73bfe6c`), CUDA 12.4, MIT | `llama-server.exe` | deep (PTQ1_0 only loads here) |

Upstream b11756 does not know the PTQ1_0 tensor type (143), so the fork is required for Bonsai.

## Measured on the target (release 1, main model)

- Health ready in about 11 s after start.
- VRAM with Qwen loaded: 5443 MiB of 8188 MiB.
- Server process memory: about 3.5 GB.
- Host free RAM with the server running: 382 MB of 16 GB (WSL VM also holds about 3.5 GB). This is why the
  prompt cache is capped at 1 GB and `parallel` is 1.
- Generation speed observed in the server log: about 42 tokens/s.

Estimates from the GGUF headers (not measured under load): Qwen at 16K context about 5.9 GiB VRAM; Bonsai at 16K
about 6.7 GiB; both together about 12.9 GiB, so they cannot run at the same time on 8 GB. Bonsai at 24K with an f16
KV cache would not fit.

## Speculative decoding (MTP)

Neither local GGUF contains an MTP head, so MTP is not used. `agent health` reports "speculative decoding active"
only for a real draft type, and reports off for `"none"`.
