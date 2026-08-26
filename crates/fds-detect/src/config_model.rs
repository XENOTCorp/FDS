//! The engine configuration surface as data: one table drives config
//! emission, JSON Schema generation, and validation, so the three cannot
//! drift from each other or from the engine's serde model
//! (`crates/fds-core/src/config.rs`). Every field carries the rationale
//! the standard requires: description, trade-off, default, and decision
//! origin (IO-04: no knob is set "because it is usually faster").

use serde_json::{json, Map, Value};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum JsonType {
    Bool,
    /// `usize` in the engine (non-negative).
    Int,
    /// `u32` in the engine (non-negative).
    U32,
    /// `i32` in the engine.
    I32,
    Str,
    /// `ReactorStrategy`, kebab-case enum.
    Strategy,
}

pub(crate) struct FieldDef {
    pub key: &'static str,
    pub json_type: JsonType,
    /// JSON literal of the engine default (see `Config::default()`).
    pub default_json: &'static str,
    /// Where the default comes from: a decision matrix or an engine ruling.
    pub derived_from: &'static str,
    pub description: &'static str,
    pub trade_off: &'static str,
}

pub(crate) struct SectionDef {
    pub name: &'static str,
    pub fields: &'static [FieldDef],
}

static CORE_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "pin_cores",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default ([CONC] pin-per-core)",
        description: "Pin each worker thread to its own logical CPU.",
        trade_off: "Removes scheduler migration and cache thrash; consumes whole CPUs. Disable when cores are shared with other tenants.",
    },
    FieldDef {
        key: "threads",
        json_type: JsonType::Int,
        default_json: "0",
        derived_from: "engine default (0 = one per logical CPU; 2x physical on SMT)",
        description: "Worker thread count; 0 = one per logical CPU.",
        trade_off: "More workers give more parallelism but per-core caches and queues must fit; oversubscription adds scheduling noise (see the measured p999 tails in WIKI.md).",
    },
    FieldDef {
        key: "stack_bytes",
        json_type: JsonType::Int,
        default_json: "1048576",
        derived_from: "engine default",
        description: "Worker thread stack size, in bytes.",
        trade_off: "Larger stacks waste address space; smaller risks overflow in deep call paths (pool buffers are heap-backed).",
    },
];

static REACTOR_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "strategy",
        json_type: JsonType::Strategy,
        default_json: "epoll-busy-poll",
        derived_from: "D-5 (polling strategy)",
        description: "Polling strategy: epoll-busy-poll (classic readiness loop, busy-poll to empty) or io-uring (kernel-side submission/completion batching).",
        trade_off: "epoll is the proven baseline; io-uring amortizes syscalls at high packet rates but its SQPOLL thread needs privileges and newer kernels for best results.",
    },
    FieldDef {
        key: "max_events",
        json_type: JsonType::Int,
        default_json: "256",
        derived_from: "engine default",
        description: "Preallocated event array capacity per reactor.",
        trade_off: "Too small drops events per drain; too large wastes memory per worker.",
    },
    FieldDef {
        key: "busy_poll",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default (latency target)",
        description: "Busy-poll the ready queue to empty before yielding.",
        trade_off: "Lowest latency at the cost of CPU spin when idle; disable on shared CPUs.",
    },
    FieldDef {
        key: "timeout_ms",
        json_type: JsonType::I32,
        default_json: "0",
        derived_from: "engine default",
        description: "Poll timeout in milliseconds when not busy-polling.",
        trade_off: "0 with busy-poll off blocks indefinitely; a small timeout bounds wake latency at the cost of syscall rate.",
    },
    FieldDef {
        key: "io_uring_entries",
        json_type: JsonType::U32,
        default_json: "256",
        derived_from: "engine default",
        description: "io_uring ring entries (strategy io-uring).",
        trade_off: "Larger rings hold more in-flight ops; entries are power-of-two and floored by the datapath (UDP slots + accept + timeout).",
    },
    FieldDef {
        key: "io_uring_sq_thread",
        json_type: JsonType::U32,
        default_json: "0",
        derived_from: "engine default",
        description: "io_uring SQPOLL thread CPU; 0 = no SQPOLL (plain ring).",
        trade_off: "SQPOLL removes submission syscalls but needs CAP_SYS_ADMIN; the ring falls back to a plain ring when creation is rejected.",
    },
];

static UDP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "rcvbuf",
        json_type: JsonType::Int,
        default_json: "4194304",
        derived_from: "D-1 (L3-aware buffer sizing; fds-detect derives from measured L3)",
        description: "SO_RCVBUF per UDP socket, in bytes.",
        trade_off: "Larger absorbs bursts and jumbo datagrams at the cost of memory; sized from the L3 budget by fds-detect.",
    },
    FieldDef {
        key: "sndbuf",
        json_type: JsonType::Int,
        default_json: "4194304",
        derived_from: "D-1 (L3-aware buffer sizing)",
        description: "SO_SNDBUF per UDP socket, in bytes.",
        trade_off: "Larger absorbs bursts at the cost of memory; the kernel doubles the value it reports.",
    },
    FieldDef {
        key: "gso_segment_size",
        json_type: JsonType::Int,
        default_json: "0",
        derived_from: "engine default",
        description: "UDP_SEGMENT (GSO) max segment size; 0 = off.",
        trade_off: "Fewer, larger sends amortize syscalls but the kernel must segment each; 0 keeps software segmentation.",
    },
    FieldDef {
        key: "gro",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "engine default",
        description: "UDP_GRO coalescing.",
        trade_off: "Fewer receive completions at the cost of per-flow reassembly state.",
    },
    FieldDef {
        key: "zerocopy",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "D-6 (copy strategy)",
        description: "MSG_ZEROCOPY for large UDP datagrams.",
        trade_off: "Avoids a kernel copy for big buffers at the cost of completion-notification handling and page pinning.",
    },
    FieldDef {
        key: "reuseport",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default ([CONC] per-core distribution)",
        description: "SO_REUSEPORT (one socket per core).",
        trade_off: "Enables per-core distribution; the option must be set before bind for reuseport-group admission (the engine does this).",
    },
    FieldDef {
        key: "incoming_cpu",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "engine ruling (loopback collapse)",
        description: "SO_INCOMING_CPU steering.",
        trade_off: "Pins flows to the RX softirq CPU — on loopback that collapses every flow onto one worker; enable only with NIC RSS/IRQ affinity (WIKI.md).",
    },
];

static TCP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "nodelay",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default (latency target)",
        description: "TCP_NODELAY.",
        trade_off: "Disables Nagle; tiny writes become small packets, correct for a latency-bound echo.",
    },
    FieldDef {
        key: "quickack",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "engine default",
        description: "TCP_QUICKACK.",
        trade_off: "Fewer delayed-ACK stalls at the cost of more ACK traffic.",
    },
    FieldDef {
        key: "defer_accept",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "engine default",
        description: "TCP_DEFER_ACCEPT.",
        trade_off: "Defers accept until data arrives, saving a wakeup on idle connections; can delay connection-establishment visibility.",
    },
    FieldDef {
        key: "fastopen",
        json_type: JsonType::U32,
        default_json: "0",
        derived_from: "engine default (SEC: spoofing caveat)",
        description: "TCP_FASTOPEN queue length; 0 = off.",
        trade_off: "Saves an RTT on repeat connections but is spoofable and needs a server-side queue budget.",
    },
    FieldDef {
        key: "cork",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "engine default",
        description: "TCP_CORK.",
        trade_off: "Coalesces small writes into full segments at the cost of added per-write latency; wrong for streaming echo.",
    },
    FieldDef {
        key: "reuseport",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default ([CONC] per-core distribution)",
        description: "SO_REUSEPORT (one listener socket per core).",
        trade_off: "Enables per-core distribution; set before bind.",
    },
    FieldDef {
        key: "rcvbuf",
        json_type: JsonType::Int,
        default_json: "4194304",
        derived_from: "D-1 (L3-aware buffer sizing)",
        description: "SO_RCVBUF per TCP socket, in bytes.",
        trade_off: "Larger absorbs bursts at the cost of memory; the kernel doubles the value it reports.",
    },
    FieldDef {
        key: "sndbuf",
        json_type: JsonType::Int,
        default_json: "4194304",
        derived_from: "D-1 (L3-aware buffer sizing)",
        description: "SO_SNDBUF per TCP socket, in bytes.",
        trade_off: "Larger absorbs bursts at the cost of memory.",
    },
];

static SCTP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "nodelay",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default",
        description: "SCTP_NODELAY (disable Nagle per association).",
        trade_off: "Lowest latency at the cost of more small packets.",
    },
    FieldDef {
        key: "init_max_streams",
        json_type: JsonType::U32,
        default_json: "10",
        derived_from: "engine default",
        description: "SCTP_INITMSG max streams (in/out).",
        trade_off: "More streams support more concurrent message flows per association at the cost of per-stream state.",
    },
    FieldDef {
        key: "partial_delivery_point",
        json_type: JsonType::U32,
        default_json: "0",
        derived_from: "engine default (0 = kernel default)",
        description: "SCTP_PARTIAL_DELIVERY_POINT, in bytes.",
        trade_off: "Smaller delivers partial messages sooner (latency) at the cost of fragmented reads.",
    },
    FieldDef {
        key: "max_burst",
        json_type: JsonType::U32,
        default_json: "0",
        derived_from: "engine default (0 = kernel default)",
        description: "SCTP_MAX_BURST; 0 = kernel default.",
        trade_off: "Limits packet bursts per association (loss avoidance) at the cost of throughput bursts.",
    },
    FieldDef {
        key: "reuseport",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "engine default ([CONC] per-core distribution)",
        description: "SO_REUSEPORT (one socket per core).",
        trade_off: "Enables per-core distribution; set before bind.",
    },
];

static METRICS_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "socket_path",
        json_type: JsonType::Str,
        default_json: "/tmp/fds-metrics.sock",
        derived_from: "engine default ([OBS] pull, not hot-path write)",
        description: "Unix socket path for the metrics pull endpoint; empty = disabled.",
        trade_off: "Pull-based metrics avoid hot-path writes; a filesystem path is a local trust boundary.",
    },
];

static ZERO_COPY_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "splice",
        json_type: JsonType::Bool,
        default_json: "true",
        derived_from: "D-6 (copy strategy)",
        description: "sendfile/splice for file-backed TCP responses.",
        trade_off: "Kernel-side copy avoids a userspace bounce for file data; costs a pipe staging when splicing between sockets.",
    },
    FieldDef {
        key: "registered_buffers",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "D-6 (copy strategy)",
        description: "io_uring registered buffers (strategy io-uring).",
        trade_off: "Avoids per-op buffer pinning at the cost of registration setup and fixed-buffer bookkeeping.",
    },
    FieldDef {
        key: "udp_zerocopy",
        json_type: JsonType::Bool,
        default_json: "false",
        derived_from: "D-6 (copy strategy)",
        description: "MSG_ZEROCOPY for UDP large datagrams (see also udp.zerocopy).",
        trade_off: "Avoids a kernel copy for big buffers at the cost of completion-notification handling and page pinning.",
    },
];

static AF_XDP_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "device",
        json_type: JsonType::Str,
        default_json: "",
        derived_from: "engine default (device-gated)",
        description: "Device name for AF_XDP (e.g. eth0); empty = disabled.",
        trade_off: "Kernel-bypass RX/TX on a dedicated queue needs an XDP-capable NIC, CAP_NET_RAW, and a pinned XDP program; absent a device the engine logs and stays on the kernel datapath.",
    },
    FieldDef {
        key: "queue",
        json_type: JsonType::U32,
        default_json: "0",
        derived_from: "engine default",
        description: "Queue id on the AF_XDP device.",
        trade_off: "Binding a queue dedicates it to the XDP path.",
    },
];

static ENGINE_FIELDS: &[FieldDef] = &[
    FieldDef {
        key: "udp_bind",
        json_type: JsonType::Str,
        default_json: "127.0.0.1:7777",
        derived_from: "engine default",
        description: "UDP echo bind address (ip:port).",
        trade_off: "Loopback default; bind 0.0.0.0 (or a NIC address) for external traffic.",
    },
    FieldDef {
        key: "tcp_bind",
        json_type: JsonType::Str,
        default_json: "127.0.0.1:7778",
        derived_from: "engine default",
        description: "TCP echo bind address (ip:port).",
        trade_off: "Loopback default; bind 0.0.0.0 for external traffic.",
    },
];

pub(crate) static SECTIONS: &[SectionDef] = &[
    SectionDef { name: "core", fields: CORE_FIELDS },
    SectionDef { name: "reactor", fields: REACTOR_FIELDS },
    SectionDef { name: "udp", fields: UDP_FIELDS },
    SectionDef { name: "tcp", fields: TCP_FIELDS },
    SectionDef { name: "sctp", fields: SCTP_FIELDS },
    SectionDef { name: "metrics", fields: METRICS_FIELDS },
    SectionDef { name: "zero_copy", fields: ZERO_COPY_FIELDS },
    SectionDef { name: "af_xdp", fields: AF_XDP_FIELDS },
    SectionDef { name: "engine", fields: ENGINE_FIELDS },
];

/// D-1 socket-buffer sizing: each socket buffer absorbs one L3-sized burst
/// while the working set stays cache-resident.
/// `clamp(pow2(L3/2), 4 MiB, 16 MiB)` — a power of two so the kernel's
/// reported (doubled) value and the ring layout stay aligned.
pub(crate) fn d1_socket_buffer_bytes(l3: u64) -> u64 {
    (l3 / 2).next_power_of_two().clamp(4 << 20, 16 << 20)
}

fn field_value(f: &FieldDef) -> Value {
    match f.json_type {
        JsonType::Bool | JsonType::Int | JsonType::U32 | JsonType::I32 => {
            serde_json::from_str(f.default_json).unwrap_or_else(|_| json!(f.default_json))
        }
        JsonType::Str | JsonType::Strategy => json!(f.default_json),
    }
}

/// Emit the repo-root `config.json` with engine defaults, except socket
/// buffers which are D-1-derived from the detected L3 size.
pub(crate) fn emit_config(l3_bytes: Option<u64>) -> String {
    let mut root = Map::new();
    for section in SECTIONS {
        let mut obj = Map::new();
        for f in section.fields {
            obj.insert(f.key.to_string(), field_value(f));
        }
        if matches!(section.name, "udp" | "tcp") {
            if let Some(l3) = l3_bytes {
                let buf = d1_socket_buffer_bytes(l3);
                for k in ["rcvbuf", "sndbuf"] {
                    obj.insert(k.to_string(), json!(buf));
                }
            }
        }
        root.insert(section.name.to_string(), Value::Object(obj));
    }
    let mut s = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    s.push('\n');
    s
}

/// Generate `config/config.schema.json` (JSON Schema, draft 2020-12) from
/// the same table the validator and emitter use. Every field carries
/// `description`, `x-trade-off`, `x-derived-from`, and `default`.
pub(crate) fn generate_schema() -> String {
    let mut properties = Map::new();
    for section in SECTIONS {
        let mut fields = Map::new();
        for f in section.fields {
            let jt = match f.json_type {
                JsonType::Bool => "boolean",
                JsonType::Int | JsonType::U32 | JsonType::I32 => "integer",
                JsonType::Str | JsonType::Strategy => "string",
            };
            let mut fd = Map::new();
            fd.insert("type".to_string(), json!(jt));
            if f.json_type == JsonType::Strategy {
                fd.insert("enum".to_string(), json!(["epoll-busy-poll", "io-uring"]));
            }
            fd.insert("default".to_string(), field_value(f));
            fd.insert("description".to_string(), json!(f.description));
            fd.insert("x-trade-off".to_string(), json!(f.trade_off));
            fd.insert("x-derived-from".to_string(), json!(f.derived_from));
            fields.insert(f.key.to_string(), Value::Object(fd));
        }
        let mut sd = Map::new();
        sd.insert("type".to_string(), json!("object"));
        sd.insert("additionalProperties".to_string(), json!(false));
        sd.insert("properties".to_string(), Value::Object(fields));
        properties.insert(section.name.to_string(), Value::Object(sd));
    }
    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://fds.local/schemas/config.schema.json",
        "title": "FDS engine configuration",
        "description": "The single repo-root config.json consumed by the fds engine. Every field is optional: missing fields fall back to the engine defaults (crates/fds-core/src/config.rs). Hardware-derived defaults are produced by fds-detect (D-1 socket-buffer sizing from measured L3; worker count follows logical CPUs).",
        "type": "object",
        "additionalProperties": false,
        "properties": Value::Object(properties),
    });
    let mut s = serde_json::to_string_pretty(&schema).unwrap_or_default();
    s.push('\n');
    s
}

/// Validate a `config.json` document against the engine surface. Missing
/// sections/fields are fine (engine defaults apply); unknown keys and type
/// mismatches are errors. Returns a list of human-readable problems.
pub(crate) fn validate_config(text: &str) -> Vec<String> {
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return vec![format!("config.json: {e}")],
    };
    let mut errors = Vec::new();
    let Some(obj) = v.as_object() else {
        return vec!["config.json: root must be an object".to_string()];
    };
    for (sec, val) in obj {
        let Some(section) = SECTIONS.iter().find(|s| s.name == sec) else {
            errors.push(format!("config.json: unknown section {sec:?}"));
            continue;
        };
        let Some(sobj) = val.as_object() else {
            errors.push(format!("config.json: section {sec:?} must be an object"));
            continue;
        };
        for (key, fv) in sobj {
            let Some(field) = section.fields.iter().find(|f| f.key == key) else {
                errors.push(format!("config.json: unknown key {sec}.{key}"));
                continue;
            };
            let ok = match field.json_type {
                JsonType::Bool => fv.is_boolean(),
                JsonType::Int | JsonType::U32 => fv.as_u64().is_some(),
                JsonType::I32 => fv.as_i64().is_some(),
                JsonType::Str => fv.is_string(),
                JsonType::Strategy => {
                    fv.as_str().is_some_and(|s| matches!(s, "epoll-busy-poll" | "io-uring"))
                }
            };
            if !ok {
                errors.push(format!("config.json: {sec}.{key}: expected {}, got {fv}", match field.json_type {
                    JsonType::Bool => "a boolean",
                    JsonType::Int => "a non-negative integer",
                    JsonType::U32 => "a non-negative integer",
                    JsonType::I32 => "an integer",
                    JsonType::Str => "a string",
                    JsonType::Strategy => "\"epoll-busy-poll\" | \"io-uring\"",
                }));
            }
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d1_buffer_rule() {
        assert_eq!(d1_socket_buffer_bytes(3 << 20), 4 << 20); // 3 MiB L3 (this host) -> default
        assert_eq!(d1_socket_buffer_bytes(8 << 20), 4 << 20);
        assert_eq!(d1_socket_buffer_bytes(16 << 20), 8 << 20);
        assert_eq!(d1_socket_buffer_bytes(24 << 20), 16 << 20);
        assert_eq!(d1_socket_buffer_bytes(64 << 20), 16 << 20); // capped
    }

    #[test]
    fn emit_matches_engine_defaults_for_small_l3() {
        // 3 MiB L3 clamps to the 4 MiB engine default, so the emitted file
        // must equal the engine's own defaults everywhere.
        let v: Value = serde_json::from_str(&emit_config(Some(3 << 20))).unwrap();
        assert_eq!(v["udp"]["rcvbuf"], json!(4 << 20));
        assert_eq!(v["tcp"]["sndbuf"], json!(4 << 20));
        assert_eq!(v["reactor"]["strategy"], json!("epoll-busy-poll"));
        assert_eq!(v["core"]["threads"], json!(0));
        assert_eq!(v["engine"]["udp_bind"], json!("127.0.0.1:7777"));
        assert_eq!(v["metrics"]["socket_path"], json!("/tmp/fds-metrics.sock"));
    }

    #[test]
    fn emit_scales_buffers_with_l3() {
        let v: Value = serde_json::from_str(&emit_config(Some(32 << 20))).unwrap();
        assert_eq!(v["udp"]["rcvbuf"], json!(16 << 20));
        assert_eq!(v["udp"]["sndbuf"], json!(16 << 20));
        assert_eq!(v["tcp"]["rcvbuf"], json!(16 << 20));
    }

    #[test]
    fn schema_covers_every_engine_field() {
        let schema: Value = serde_json::from_str(&generate_schema()).unwrap();
        for section in SECTIONS {
            let sprops = &schema["properties"][section.name]["properties"];
            for f in section.fields {
                let fd = &sprops[f.key];
                assert!(fd["type"].is_string(), "{}:{} missing type", section.name, f.key);
                assert!(fd["description"].is_string());
                assert!(fd["x-trade-off"].is_string());
                assert!(fd["x-derived-from"].is_string());
                assert!(fd["default"].is_boolean() || fd["default"].is_number() || fd["default"].is_string());
            }
        }
    }

    #[test]
    fn validate_accepts_empty_and_full() {
        assert!(validate_config("{}").is_empty());
        assert!(validate_config(&emit_config(None)).is_empty());
        assert!(validate_config(&emit_config(Some(16 << 20))).is_empty());
    }

    #[test]
    fn validate_rejects_unknown_and_mistyped() {
        let errors = validate_config(r#"{ "core": { "bogus": 1 }, "udp": { "rcvbuf": "big" } }"#);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("unknown key core.bogus"));
        assert!(errors[1].contains("udp.rcvbuf"));

        let errors = validate_config(r#"{ "nope": {} }"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("unknown section"));

        let errors = validate_config(r#"{ "reactor": { "strategy": "busy-loop" } }"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("reactor.strategy"));

        let errors = validate_config(r#"{ "core": 7 }"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("section \"core\" must be an object"));
    }

    #[test]
    fn validate_rejects_malformed_json() {
        let errors = validate_config("{ nope");
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn negative_timeout_ok_but_negative_buffers_rejected() {
        // timeout_ms is i32 in the engine; buffers are usize.
        assert!(validate_config(r#"{ "reactor": { "timeout_ms": -1 } }"#).is_empty());
        let errors = validate_config(r#"{ "udp": { "rcvbuf": -1 } }"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("udp.rcvbuf"));
    }
}
