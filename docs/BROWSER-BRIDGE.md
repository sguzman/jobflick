# Browser bridge protocol

A Chromium Native Messaging host runs as jobflick native-host. Each request and response is exactly one UTF-8 JSON document preceded by a 32-bit little-endian payload length. The maximum request size is the 256-KiB Execarium limit plus 8 KiB.

- validate_spec: spec_yaml; performs strict Execarium + Jobflick consumer validation, does not run work.
- submit_spec: spec_yaml and optional origin_tab_id; enqueues exactly once, returns the persisted job including ID, name and color.
- inspect: id; returns state and metadata, without requiring completion.
- show: id; returns completed report, or an error for unfinished work.
- cancel: id; explicit cancellation.

The browser *creates* origin_tab_id. It is a printable opaque 1–128 character token of ASCII letters, digits, hyphen or underscore. It is not part of the YAML specification and is not a browser ID trusted for authorization. The client is responsible for retaining tab/conversation-to-job mappings and ensuring that only the proper chat receives a completed report.

No automatic retry of a submitted request is permitted after uncertain disconnect: the daemon might have enqueued it. Consumers must reconcile by durable job ID or user intervention. The host's stdout must remain binary framing only.
