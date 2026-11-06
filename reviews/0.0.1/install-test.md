# goose-triggers 0.0.1: clean-install check

The README's install and use steps, run as written against the v0.0.1 release on 6 November 2026 by the automated build, in a clean container (Debian bookworm-slim, goose 1.57.0, HOME=/tmp, no model provider, no API key). Result: install, sha256 check and selftest (20 checks) pass; the example rules start runs, which fail with goose's own error, as there is no provider. This check was run by the automated build itself; from 0.0.2 on, each release has a separate automated tester's report (`../0.0.2/install-test.md`). Its shell trace:

```
+ cd /tmp
+ curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.1/goose-triggers-v0.0.1-x86_64-linux.tar.gz
+ curl -fsSLO https://github.com/alepacheco/goose-triggers/releases/download/v0.0.1/goose-triggers-v0.0.1-x86_64-linux.tar.gz.sha256
+ sha256sum -c goose-triggers-v0.0.1-x86_64-linux.tar.gz.sha256
goose-triggers-v0.0.1-x86_64-linux.tar.gz: OK
+ tar -xzf goose-triggers-v0.0.1-x86_64-linux.tar.gz
+ mkdir -p /tmp/.local/bin
+ install -m 755 goose-triggers-v0.0.1-x86_64-linux/goose-triggers /tmp/.local/bin/
+ export PATH=/tmp/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
+ PATH=/tmp/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
+ goose-triggers --version
goose-triggers 0.0.1
+ goose-triggers selftest
+ tail -2
  ok  goose-triggers run stopped
selftest passed: 20 checks
+ mkdir -p /tmp/.config/goose-triggers/recipes /tmp/Inbox
+ cd /tmp/.config/goose-triggers
+ base=https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.1/examples
+ curl -fsSL -o config.yaml https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.1/examples/config.yaml
+ curl -fsSL -o recipes/file-note.yaml https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.1/examples/recipes/file-note.yaml
+ curl -fsSL -o recipes/form-entry.yaml https://raw.githubusercontent.com/alepacheco/goose-triggers/v0.0.1/examples/recipes/form-entry.yaml
+ umask 077
+ head -c 24 /dev/urandom
+ base64
+ ls -la /tmp/.config/goose-triggers
total 5
drwxr-xr-x 3 1000 1000    5 Nov  6 01:13 .
drwxr-xr-x 3 1000 1000    3 Nov  6 01:13 ..
-rw-r--r-- 1 1000 1000 1498 Nov  6 01:13 config.yaml
drwxr-xr-x 2 1000 1000    4 Nov  6 01:13 recipes
-rw------- 1 1000 1000   33 Nov  6 01:13 webhook-secret
++ cat /tmp/.config/goose-triggers/webhook-secret
+ export GOOSE_TRIGGERS_WEBHOOK_SECRET=<the generated secret>
+ GOOSE_TRIGGERS_WEBHOOK_SECRET=<the generated secret>
+ sleep 3
+ goose-triggers run
+ echo 'a PDF for the test'
+ curl -sS -X POST http://127.0.0.1:8787/hooks/form-entry -H 'Authorization: Bearer <the generated secret>' -d '{"name": "Test Person", "email": "test@example.com"}'
{"event":1,"run":1}+ echo

+ sleep 8
+ goose-triggers status
  RUN  RULE                 STATE     UPDATED              SESSION        DETAIL
    2  inbox-notes          failed    2026-11-06 01:13:53  -              goose exited with exit status: 1: file_path: /tmp/Inbox/test.pdf / file_name: test.pdf / error: No …
    1  form-entry           failed    2026-11-06 01:13:52  -              goose exited with exit status: 1: email: test@example.com / name: Test Person / error: No provider …
+ goose-triggers log 1
+ head -30
run 1: rule form-entry, failed (since 2026-11-06 01:13:52, created 2026-11-06 01:13:51)
error: goose exited with exit status: 1: email: test@example.com / name: Test Person / error: No provider configured. Run 'goose configure' first.; see goose-triggers log 1
recipe: /tmp/.config/goose-triggers/recipes/form-entry.yaml
event 1 (webhook, 2026-11-06 01:13:51):
  body = {"name": "Test Person", "email": "test@example.com"}
  email = test@example.com
  name = Test Person
goose session: -
working folder: /tmp/.local/share/goose-triggers/work/form-entry

timeline:
  2026-11-06 01:13:51  webhook event 1: email=test@example.com name=Test Person
  2026-11-06 01:13:52  goose started (pid 188) in /tmp/.local/share/goose-triggers/work/form-entry
  2026-11-06 01:13:52  failed: goose exited with exit status: 1: email: test@example.com / name: Test Person / error: No provider configured. Run 'goose configure' first.; see goose-triggers log 1

goose (/tmp/.local/share/goose-triggers/runs/1.jsonl):
  goose said: error: No provider configured. Run 'goose configure' first.

goose's stderr, last lines (/tmp/.local/share/goose-triggers/runs/1.stderr):
  Loading recipe: Record a form entry
  Description: Appends a form entry sent by webhook to entries.csv.
  Parameters used to load this recipe:
+ kill -TERM %1
+ wait
+ cat /tmp/run.log
[2026-11-06 01:13:48] rule inbox-notes: new files matching "*.pdf" in /tmp/Inbox
[2026-11-06 01:13:48] rule form-entry: POST http://127.0.0.1:8787/hooks/form-entry
[2026-11-06 01:13:48] state in /tmp/.local/share/goose-triggers
[2026-11-06 01:13:51] run 1 (form-entry): queued for webhook event 1: email=test@example.com name=Test Person
[2026-11-06 01:13:52] run 1 (form-entry): goose started (pid 188) in /tmp/.local/share/goose-triggers/work/form-entry
[2026-11-06 01:13:52] run 1 (form-entry): failed: goose exited with exit status: 1: email: test@example.com / name: Test Person / error: No provider configured. Run 'goose configure' first.; see goose-triggers log 1
[2026-11-06 01:13:52] run 2 (inbox-notes): queued for folder event 2: file_name=test.pdf file_path=/tmp/Inbox/test.pdf folder=/tmp/Inbox
[2026-11-06 01:13:53] run 2 (inbox-notes): goose started (pid 207) in /tmp/.local/share/goose-triggers/work/inbox-notes
[2026-11-06 01:13:53] run 2 (inbox-notes): failed: goose exited with exit status: 1: file_path: /tmp/Inbox/test.pdf / file_name: test.pdf / error: No provider configured. Run 'goose configure' first.; see goose-triggers log 2
[2026-11-06 01:14:00] stopping
[2026-11-06 01:14:00] stopped
```
