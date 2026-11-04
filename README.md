# Mitochondria

Codename for an experimental language whose memory is a **declared tree of occupancy-aware regions**.
Stasis taught that program state should be a visible, fixed model. Mitochondria asks the next question:
can the runtime also *know what is in use right now*, not only what was reserved?

Status: runtime-memory experiment only. There is no parser yet. See [DESIGN.md](DESIGN.md) for the
claims and the first measurements. Run them with `cargo run --release` (Windows only for now).
