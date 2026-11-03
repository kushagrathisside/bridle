# Serve the models

Bridle uses model servers that you start on this machine. This page lists the model files it was tested with, where to
download them, how to check them, and how to start, check and stop each server. The slots the files fill are described in
[models and slots](../concepts/models.md).

## The model files

| Slot | Model | Quantisation | Size | Source (repository and revision) | File | Licence |
|---|---|---|---|---|---|---|
| `main` | Qwen3.5-9B | Q4_K_M | 5.68 GB | `unsloth/Qwen3.5-9B-GGUF` at `3885219b6810b007914f3a7950a8d1b469d598a5` | `Qwen3.5-9B-Q4_K_M.gguf` | Apache-2.0 |
| `micro` | LFM2.5-1.2B-Thinking | Q4_K_M | 0.73 GB | `LiquidAI/LFM2.5-1.2B-Thinking-GGUF` at `9584426effbfa44b677e925793a12188a36b44ce` | `LFM2.5-1.2B-Thinking-Q4_K_M.gguf` | LFM Open License v1.0, research use |
| `deep` | Ternary Bonsai 2 27B | PTQ1_0 | 5.95 GB | `prism-ml/Ternary-Bonsai-2-27B-gguf` at `b072e1d3b35a0a630cece372c2127528e0994386` | `Ternary-Bonsai-2-27B-PTQ1_0.gguf` | Apache-2.0 |

The model files are not in the repository, and Bridle does not download them. Download each file from its source, into the
weights folder you gave `bridle init --machine`, under the folder names in the file column:

```text
<models-dir>\Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf
<models-dir>\LFM2.5-1.2B-Thinking\LFM2.5-1.2B-Thinking-Q4_K_M.gguf
<models-dir>\Ternary-Bonsai-2-27B\Ternary-Bonsai-2-27B-PTQ1_0.gguf
```

The `micro` and `deep` files are optional. The `main` file is needed for the agent.

## Check the checksums

Each file's SHA-256 is recorded in the project configuration, and Bridle compares it when it writes a manifest. Compute the
hash of the file you downloaded:

```bat
certutil -hashfile <models-dir>\Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf SHA256
```

The value must match the one in the profile, `03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8` for the
`main` file. The other two values are `7223a2202405b02e8e1e6c5baa543c43dc98c1d9741a5c2a0ee1583212e1231b` for `micro` and
`53107f530aa52eb00912263ab1ee29bd199261c87cd7b4ad4ca1318c1fe33ee3` for `deep`. Write a manifest that records what was
run:

```bat
bridle models manifest main
```

## Inference runtimes

| Runtime | Build | Used for |
|---|---|---|
| llama.cpp | b11756, CUDA 12.4 | `main` and `micro` |
| PrismML llama.cpp fork | `prism-b10787-73bfe6c`, CUDA 12.4 | `deep`. The PTQ1_0 weights load only in this fork. |

Put each runtime in the runtimes folder, under the paths the profile expects:
`llama.cpp\b11756-cuda12.4\llama-server.exe` and `llama.cpp-prism\prism-b10787-cuda12.4\llama-server.exe`.

## Start, check and stop a server

```bat
bridle models serve main --detach
bridle health main
bridle models status
bridle models stop main
```

- `serve` starts the server. With `--detach` it returns once the server answers its health check, and the server keeps
  running. `--wait-secs` sets how long to wait, and the default is 300.
- `health main` runs the checks against the slot. The last line is the result. A running `main` server ends with
  `result: pass`.
- `models status` shows each slot as `unloaded`, `warming`, `ready` or `error`.
- `stop` ends a server that was started with `--detach`.

Bridle runs one GPU model at a time. `serve` refuses to start a second GPU server while another is running.

## Measured on the tested machine

These figures were measured on 2026-10-21 on the tested machine, with the `main` model. They are not a guarantee for other
hardware.

| Measure | Value |
|---|---|
| Time from start to a healthy server | About 11 seconds |
| GPU memory with the model loaded | 5443 MiB of 8188 MiB |
| Server process memory | About 3.5 GB |
| Free system memory with the server running | About 382 MB of 16 GB |
| Generation speed reported by the server | About 42 tokens per second |

The free-memory figure is why the configuration caps the prompt cache at 1 GB and runs one server slot. Loading the `deep`
model requires stopping the `main` model first; Bridle does this for you when deep escalation is enabled.

## Known limits

- The `main` and `deep` models do not fit in 8 GB of GPU memory together.
- The `deep` model's PTQ1_0 weights require the PrismML fork.
- Speculative decoding is not used: none of the local files contains the draft head it needs.
- The `micro` model's licence is for research use.

## If a server does not start

See [troubleshooting](troubleshooting.md).

## Vision projector and context size (2026-11-03)

- **Projector.** `Qwen3.5-9B/mmproj-F16.gguf`, 918,166,080 bytes, SHA-256
  `f70dc3509053962b0d0d3ee8a7eacebf5d60aa560cad78254ae8698516ae029f`, from `unsloth/Qwen3.5-9B-GGUF` at revision
  `3885219b6810b007914f3a7950a8d1b469d598a5`. Set `launch.mmproj_file`; the server then starts with `--mmproj`, and with
  `--no-mmproj-offload` when `launch.mmproj_offload = false`.
- **Cost of the cache.** This model keeps a cache in only 8 of its 32 layers (the others hold a fixed state). With an 8-bit
  cache that is 17,408 bytes per token, read from the file's header.
- **Measured on the RTX 4060 8 GB** (used VRAM including about 230 MiB of desktop): 16K 5,455 MiB; 98K with the projector on the
  GPU 7,887 MiB (70 MiB free; the machine fell over in that state, see progress); 98K with the projector in system memory
  7,203 MiB; **64K with the projector in system memory 6,499 MiB (1,458 MiB free)**, the default.
- **The guard.** A GPU server starts only if its estimate plus 768 MiB fits in the free VRAM.
