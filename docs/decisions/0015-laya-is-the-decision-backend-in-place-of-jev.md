# ADR-0015: Laya is the decision backend in place of Jev

- **Status:** Accepted (2026-10-31) by the owner's choice. Installed, measured and wired on 2026-10-31 (progress.md): the
  client is in `harness-runtime::laya`, the configuration is `[external.laya]` and `[routing] auto_slot`, and the routing
  is used by the chat and by `agent route`. Not yet shown on the terminal screen; the deep slot's serving is untested.
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

ADR-0007 made Jev, TypeSafe AI's decision model, an optional backend for stalled loops, behind the endpoint policy.
Jev is a closed, cloud-only service, so it cannot run on this machine and it sends the decision state off the machine.

The owner asked for an open-source replacement that runs locally. Laya was chosen from the open alternatives listed in
[DataCamp's review of Jev alternatives](https://www.datacamp.com/blog/top-open-source-jev-alternatives).

Laya, as its model card describes it ([convaiinnovations/laya](https://huggingface.co/convaiinnovations/laya)):

- A non-autoregressive decision model. It returns typed answers (a choice, a score, or a yes/no probability) from one
  forward pass, not generated text.
- Apache 2.0, public weights, Python 3.10 or newer. Installed with `pip install laya`; the HTTP server is `laya[serve]`.
- An HTTP server, `laya-serve`, with `POST /v1/systemone`. The request has `state` and `questions`. Its response has the
  same shape as Jev's, according to the publisher, but the raw body is not shown on the card.
- Three checkpoints. The typed-decisions checkpoint (ModernBERT-large, 1024 tokens) is the one for decisions.
- CPU inference is supported. The card reports 193 to 464 ms for its router on CPU.

These are the publisher's figures. The comparison with Jev is not controlled, and the card says its zero-shot baseline is
near chance on typed decisions (0.362); the 0.766 figure is for the fine-tuned checkpoint.

## Decision

1. **Laya replaces Jev as the optional decision backend** (ADR-0007). Jev remains only as a documented option that the
   endpoint policy still governs.
2. **Laya runs locally.** Its server binds to loopback. The endpoint policy (V4, V32) already accepts loopback endpoints
   without cloud permission.
3. **Bridle keeps its own client.** The request and response shapes are fixed by the real server, so the client is written
   against the responses seen in a live run, not against the card. Until then no adapter is built.
4. **Laya is also a candidate router.** A typed question with the choices `main` and `deep` can route each request. This
   is part of multi-model routing (ADR-0010) and is measured the same way as the stalled-loop decision.

## Consequences

- The LFM models stay as the stalled-loop decision models until a Laya run replaces them on the same cases.
- Laya's Python environment is a separate install on each machine. The Linux machine needs Python 3.10 or newer and
  a CPU build of PyTorch.
- The install and the model download come from outside the machine, so they need the owner's go-ahead on each machine.

## Open

- Install Laya and fetch the typed-decisions checkpoint, then record the real request and response shapes.
- Measure Laya on the stalled-loop cases and on a routing set, with examples that are not among the test cases. Done for the
  routing set in its seed form (`evals/routing`, 19 of 20, with the caveat in ADR-0010); the examples must still be
  replaced by requests that are not written around the rule keywords.
