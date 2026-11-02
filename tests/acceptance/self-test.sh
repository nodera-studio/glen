#!/usr/bin/env bash
# Offline verification: recordings and fake executables only. Never runs setup or person; runs
# teardown only on mode m's hand-made recordings in its own scratch directory.
set -euo pipefail
A=$(cd -- "$(dirname -- "$0")" && pwd -P)
# shellcheck source=tests/acceptance/acc.sh
source "$A/acc.sh"
S=$(mktemp -d "${TMPDIR:-/tmp}/ga-fixture.XXXXXX")
S=$(cd -- "$S" && pwd -P)
trap 'rm -rf "$S"' EXIT # This script owns every file beneath its newly created directory.
mkdir -p "$S/project" "$S/bin" "$S/glen-bin"
# Evidence retains conversation lines and drops Claude's internal beta-header telemetry.
printf '%s\n' '{"type":"api-request","betas":["oauth-2025-04-20","claude-code-20250219"]}' '{"type":"user","message":{"content":"hello"}}' > "$S/transcript.jsonl"
copy_transcript "$S/transcript.jsonl" "$S/filtered-transcript.jsonl"
jq -se 'length==1 and .[0].type=="user" and .[0].message.content=="hello"' "$S/filtered-transcript.jsonl" >/dev/null
copy_transcript "$S/missing-transcript.jsonl" "$S/absent-transcript.jsonl"
[[ ! -e $S/absent-transcript.jsonl ]] || die 'missing transcript produced evidence'
# Large help output must finish writing before either required-command check exits early.
cat > "$S/help-glen" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ ${1-} == --help ]]
printf '  inbox ...\n  install-extension ...\n'
perl -e 'print "x" x 1024, "\n" for 1..200;'
EOF
chmod +x "$S/help-glen"
for iteration in {1..20}; do
    (require_w3_surface "$S/help-glen") || die "large help gate failed on iteration $iteration"
done
jq -n --arg s "$S" '{scratch:$s,project:($s+"/project"),R:{id:"11111111-1111-1111-1111-11111aaaaaaa",handle:"aaaaaaa"},L:{id:"22222222-2222-2222-2222-22222bbbbbbb",handle:"bbbbbbb"},X:{id:"33333333-3333-3333-3333-33333ccccccc",handle:"ccccccc"},NONCE_R:"R-nonce",NONCE_L:"L-nonce",NONCE_T:"T-nonce"}' > "$S/manifest.json"
jq -cn --slurpfile m "$S/manifest.json" '
  $m[0] as $m |
  [ {a:["ls"],out:{data:{sessions:[$m.R,$m.L,$m.X]}},exit:0},
    {a:["tail","aaaaaaa"],out:{data:{session:$m.R}},exit:0},
    {a:["send","aaaaaaa","hi"],out:{data:{job:{session_id:$m.R.id,route:"rpc"},assistant_text:$m.NONCE_R}},exit:0},
    {a:["send","bbbbbbb","hi"],out:{data:{job:{session_id:$m.L.id,route:"live"},replies:[{text:$m.NONCE_L}]}},exit:0},
    {a:["inbox"],out:{data:{messages:[{text:$m.NONCE_T}]}},exit:0},
    {a:["send","ccccccc","hi"],out:{error:{hint:"close that session"}},exit:6}
  ] | to_entries[] | .key as $i | .value as $v |
  {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
  {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$S/calls.ndjson"
printf 'R-nonce L-nonce T-nonce close that session\n' > "$S/report.md"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls")}}]}}' > "$S/transcript.jsonl"
# Telemetry without tools may be dropped from copied evidence.
printf '%s\n' '{"type":"api-request","betas":["oauth-2025-04-20","claude-code-20250219"]}' >> "$S/transcript.jsonl"
"$A/acc.sh" check b "$S"
cp "$S/calls.ndjson" "$S/good.ndjson"
cp "$S/transcript.jsonl" "$S/good-transcript.jsonl"
# A usage-rejected X send does not count alongside the real exit-6 refusal.
jq -c 'select(.argv[0]!="send" or .argv[1]!="ccccccc")' "$S/good.ndjson" > "$S/calls.ndjson"
jq -cn '{t:"start",id:"x-usage",argv:["send","ccccccc","hi","--cwd","."],ts:40},
  {t:"end",id:"x-usage",argv:["send","ccccccc","hi","--cwd","."],exit:2,ms:20,stdout:{error:{code:"usage"}}}' >> "$S/calls.ndjson"
# With only usage-rejected X sends, neither a refusal nor its report hint exists.
if "$A/acc.sh" check b "$S"; then echo 'usage-only X recording passed' >&2; exit 1; fi
jq -e '(.pass|not) and (.checks|all(.[]; if .id=="AC-415-X" or .id=="AC-415-report" then (.pass|not) else .pass end))' "$S/verdict.json" >/dev/null
jq -c 'select(.argv[0]=="send" and .argv[1]=="ccccccc")' "$S/good.ndjson" >> "$S/calls.ndjson"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="AC-415-X" and .pass))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# A successful X send breaks the requirement that every counted X send is refused.
jq -cn --slurpfile m "$S/manifest.json" '
  {t:"start",id:"x-success",argv:["send","ccccccc","hi"],ts:100},
  {t:"end",id:"x-success",argv:["send","ccccccc","hi"],exit:0,ms:20,
   stdout:{v:1,ok:true,data:{job:{session_id:$m[0].X.id,route:"rpc"},assistant_text:"delivered"}}}' >> "$S/calls.ndjson"
if "$A/acc.sh" check b "$S"; then echo 'successful X send recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-X" and (.pass|not)) and all(.[];.pass or .id=="AC-415-X")' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Raw inbox NDJSON strings still supply the talk-back nonce.
jq -c 'if .t=="end" and .argv[0]=="inbox" then .stdout="{\"text\":\"T-nonce\"}\n{\"text\":\"another reply\"}\n" else . end' "$S/good.ndjson" > "$S/calls.ndjson"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="nonce_T" and .pass))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Plain inbox strings without the nonce must fail the string-stdout fallback.
jq -c 'if .t=="end" and .argv[0]=="inbox" then .stdout="another reply without the nonce" else . end' "$S/good.ndjson" > "$S/calls.ndjson"
if "$A/acc.sh" check b "$S"; then echo 'inbox string without nonce passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="nonce_T" and (.pass|not)) and all(.[];.pass or .id=="nonce_T")' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Unknown line types must never hide tools, even otherwise permitted wrapper calls.
jq -cn --arg s "$S" '{type:"unknown",message:{content:[{type:"tool_use",id:"hidden",name:"Bash",input:{command:($s+"/bin/glen ls")}}]}}' >> "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'unknown transcript tool passed' >&2; exit 1; fi
jq -e '.checks|all(.[]; if .id=="AC-415-transcript" or .id=="AC-415-isolation" then (.pass|not) else .pass end)' "$S/verdict.json" >/dev/null
copy_transcript "$S/transcript.jsonl" "$S/filtered-transcript.jsonl"
jq -se 'all(.[];.type!="unknown" and .type!="api-request")' "$S/filtered-transcript.jsonl" >/dev/null
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls; echo \"exit=$?\"")}}]}}' > "$S/transcript.jsonl"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="AC-415-isolation" and .pass))' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
# Stripping the allowed echo suffix must not hide an earlier compound command.
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls && true; echo \"exit=$?\"")}}]}}' > "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'compound command before echo passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls; cat /etc/passwd")}}]}}' > "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'compound b recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls | jq .")}}]}}' > "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'piped b recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
for suffix in 'ls & cat /etc/passwd' 'ls > /tmp/out' 'ls <(cat x)' "ls \${HOME}" 'ls; ECHO "exit=$?"'; do
    jq -cn --arg command "$S/bin/glen $suffix" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:$command}}]}}' > "$S/transcript.jsonl"
    if "$A/acc.sh" check b "$S"; then echo "unsafe b recording passed: $suffix" >&2; exit 1; fi
    jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
done
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
# Quoted wrapper paths and plain or quoted arguments remain valid.
for wrapper in "$S/bin/glen" "'$S/bin/glen'" "\"$S/bin/glen\""; do
    jq -cn --arg command "$wrapper send aaaaaaa 'hello world' --as=\"auto\" --json" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:$command}}]}}' > "$S/transcript.jsonl"
    "$A/acc.sh" check b "$S"
    jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
done
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
# A help-only read, including a short-flag cluster, cannot satisfy list_and_read.
for argv in '["tail","X","-fh"]' '["tail","--help"]'; do
    jq -c --argjson argv "$argv" 'if .argv[0]=="tail" then .argv=$argv | if .t=="end" then .stdout="Usage: glen tail [OPTIONS]" else . end else . end' "$S/good.ndjson" > "$S/calls.ndjson"
    if "$A/acc.sh" check b "$S"; then echo 'help-only read recording passed' >&2; exit 1; fi
    jq -e '.checks|any(.id=="list_and_read" and (.pass|not))' "$S/verdict.json" >/dev/null
done
cp "$S/good.ndjson" "$S/calls.ndjson"
jq -c 'if .t=="end" and .stdout.data.assistant_text? then .stdout.data.assistant_text="missing" else . end' "$S/good.ndjson" > "$S/calls.ndjson"
printf '{"t":"start","id":"killed","argv":["ls"],"ts":100}\n{"t":"guard_refused","argv":["send","foreign"]}\n' >> "$S/calls.ndjson"
if "$A/acc.sh" check b "$S"; then echo 'bad recording passed' >&2; exit 1; fi
jq -e 'all(["nonce_R","paired_calls","guard_refused"][]; . as $id | any($ARGS.named.v.checks[];.id==$id and (.pass|not)))' --argjson v "$(cat "$S/verdict.json")" <<< null >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
printf '%s\n' '{"type":"tool_use","id":"skill","name":"Skill","input":{"skill":"glen"}}' '{"type":"tool_result","tool_use_id":"skill","content":"Skill loaded"}' '{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"glen ls"}}' '{"type":"tool_result","tool_use_id":"bash","content":"sessions listed"}' > "$S/b3.jsonl"
"$A/acc.sh" check b3 "$S"
jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
cp "$S/b3.jsonl" "$S/b3-good.jsonl"
# A successful glen load cannot authorize an additional, unrelated Skill call.
cp "$S/b3-good.jsonl" "$S/b3.jsonl"
printf '%s\n' '{"type":"tool_use","id":"other","name":"Skill","input":{"skill":"other-skill"}}' '{"type":"tool_result","tool_use_id":"other","content":"Skill loaded"}' >> "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'unrelated Skill recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-only-glen-skill" and (.pass|not)) and all(.[];.pass or .id=="AC-420-only-glen-skill")' "$S/verdict.json" >/dev/null
# A glen Skill result marked as an error never counts as a successful load.
jq -c 'if .tool_use_id?=="skill" then .is_error=true else . end' "$S/b3-good.jsonl" > "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'error Skill recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-loaded" and (.pass|not)) and all(.[];.pass or .id=="AC-420-loaded")' "$S/verdict.json" >/dev/null
# A denied Skill loader must not count as a discovered skill load, even without is_error.
jq -c 'if .tool_use_id?=="skill" then .content="Permission denied" else . end' "$S/b3-good.jsonl" > "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'denied Skill recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-loaded" and (.pass|not))' "$S/verdict.json" >/dev/null
jq -c 'if .tool_use_id?=="bash" then .content="Permission denied" else . end' "$S/b3-good.jsonl" > "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'denied b3 recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-permission" and (.pass|not))' "$S/verdict.json" >/dev/null
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",content:({v:1,ok:true,data:{sessions:[]}}|tojson)}]}}' > "$S/b2-probe.jsonl"
printf '%s\n' '{"type":"tool_use","id":"control","name":"Bash","input":{"command":"glen ls"}}' '{"type":"tool_result","tool_use_id":"control","content":"Permission denied"}' > "$S/b2-control.jsonl"
"$A/acc.sh" check b2 "$S"
jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
# Malformed Bash commands leave the real control call's verdict unchanged.
cp "$S/b2-control.jsonl" "$S/control-good.jsonl"
printf '%s\n' '{"type":"tool_use","id":"null-command","name":"Bash","input":{"command":null}}' '{"type":"tool_use","id":"array-command","name":"Bash","input":{"command":["glen ls"]}}' >> "$S/b2-control.jsonl"
"$A/acc.sh" check b2 "$S"
jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
cp "$S/control-good.jsonl" "$S/b2-control.jsonl"
# Text content arrays use the same JSON parsing path.
jq -c '.message.content[1].content |= [{type:"text",text:.}]' "$S/b2-probe.jsonl" > "$S/probe-array.jsonl"
cp "$S/probe-array.jsonl" "$S/b2-probe.jsonl"
"$A/acc.sh" check b2 "$S"
jq -e '.pass and (.checks|any(.id=="AC-416-probe" and .pass))' "$S/verdict.json" >/dev/null
# Text that resembles a success object must actually parse as JSON.
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",content:"not-json \"ok\": true"}]}}' > "$S/b2-probe.jsonl"
if "$A/acc.sh" check b2 "$S"; then echo 'unparseable b2 success text passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-416-probe" and (.pass|not)) and all(.[];.pass or .id=="AC-416-probe")' "$S/verdict.json" >/dev/null
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",is_error:true,content:"{\"ok\":true}"}]}}' > "$S/b2-probe.jsonl"
if "$A/acc.sh" check b2 "$S"; then echo 'error b2 result with ok:true passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-416-probe" and (.pass|not)) and all(.[];.pass or .id=="AC-416-probe")' "$S/verdict.json" >/dev/null
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",content:({v:1,ok:false,error:{message:"Permission denied"}}|tojson)}]}}' > "$S/b2-probe.jsonl"
if "$A/acc.sh" check b2 "$S"; then echo 'denied b2 recording passed' >&2; exit 1; fi
jq -e '(.pass|not) and (.checks|any(.id=="AC-416-probe" and (.pass|not)))' "$S/verdict.json" >/dev/null
# Usage rejections are not delivery attempts; sandboxed sends still count.
jq -cn --arg s "$S" '
  [ {a:["ls"],out:{data:{sessions:[]}},exit:0},
    {a:["tail","aaaaaaa"],out:{data:{}},exit:0},
    {a:["send","aaaaaaa","hi","--cwd",($s+"/project")],out:{error:{code:"usage"}},exit:2},
    {a:["send","aaaaaaa","hi"],out:{error:{hint:"sandbox hint",details:{reason:"sandboxed",network:false}}},exit:6},
    {a:["send","bbbbbbb","hi"],out:{error:{hint:"sandbox hint",details:{reason:"sandboxed",network:false}}},exit:6}
  ] | to_entries[] | .key as $i | .value as $v |
  {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
  {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$S/calls.ndjson"
"$A/acc.sh" check c1 "$S" >/dev/null || true
jq -e '.checks|any(.id=="AC-417-sandbox" and .pass)' "$S/verdict.json" >/dev/null
cp "$S/calls.ndjson" "$S/c1-mixed.ndjson"
# Failed help with an error envelope remains a delivery attempt and fails the sandbox check.
jq -cn '{t:"start",id:"failed-help",argv:["send","--help"],ts:100},
  {t:"end",id:"failed-help",argv:["send","--help"],exit:1,ms:20,stdout:{ok:false,error:{code:"internal",message:"help failed"}}}' >> "$S/calls.ndjson"
if "$A/acc.sh" check c1 "$S"; then echo 'failed help recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-417-sandbox" and (.pass|not))' "$S/verdict.json" >/dev/null
# Usage errors alone must not satisfy the nonempty delivery-attempt requirement.
jq -c 'select(.argv[0]!="send" or (.argv|index("--cwd")))' "$S/c1-mixed.ndjson" > "$S/calls.ndjson"
if "$A/acc.sh" check c1 "$S"; then echo 'usage-only c1 recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-417-sandbox" and (.pass|not))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# invalid_env is not usage: the sole send remains a failed delivery attempt.
jq -c 'select(.argv[0]!="send" or (.argv|index("--cwd"))) |
  if .t=="end" and .argv[0]=="send" then .stdout.error.code="invalid_env" else . end' "$S/c1-mixed.ndjson" > "$S/calls.ndjson"
if "$A/acc.sh" check c1 "$S"; then echo 'invalid_env-only c1 recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-417-sandbox" and (.pass|not))' "$S/verdict.json" >/dev/null
# Adding a genuine sandbox refusal would pass if invalid_env had been wrongly excluded.
jq -c 'select(.argv[0]=="send" and .argv[1]=="aaaaaaa" and (.argv|index("--cwd")|not))' "$S/c1-mixed.ndjson" >> "$S/calls.ndjson"
if "$A/acc.sh" check c1 "$S"; then echo 'invalid_env delivery attempt was ignored' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-417-sandbox" and (.pass|not))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Missing recordings must retain every mode's check IDs in a valid failing verdict.
for mode in a b b2 b3 c1 c2; do
    "$A/acc.sh" check "$mode" "$S" >/dev/null || true
    jq '[.checks[].id]' "$S/verdict.json" > "$S/check-ids.json"
    rm "$S/calls.ndjson"
    if "$A/acc.sh" check "$mode" "$S"; then echo "missing $mode recording passed" >&2; exit 1; fi
    jq -e --slurpfile ids "$S/check-ids.json" --arg path "$S/calls.ndjson" \
      '(.pass|not) and ([.checks[].id]==$ids[0]) and (.checks|length>0) and all(.checks[]; (.pass|not) and (.evidence|contains($path) and contains("missing recording")))' "$S/verdict.json" >/dev/null
    cp "$S/good.ndjson" "$S/calls.ndjson"
done
# Help stdout is plain text and must not count as a delivery attempt or other content.
mkdir -p "$S/c2-home/.codex"
printf '# offline config fixture\n' > "$S/c2-home/.codex/config.toml"
shasum -a 256 "$S/c2-home/.codex/config.toml" | cut -d ' ' -f1 > "$S/config.before.sha256"
jq -cn --slurpfile m "$S/manifest.json" '
  $m[0] as $m |
  [ {a:["ls"],out:{data:{sessions:[$m.R,$m.L,$m.X]}},exit:0},
    {a:["tail","aaaaaaa"],out:{data:{session:$m.R}},exit:0},
    {a:["send","aaaaaaa","hi"],out:{error:{details:{reason:"sandboxed",network:true}}},exit:6},
    {a:["send","bbbbbbb","hi"],out:{data:{job:{session_id:$m.L.id,route:"live"},assistant_text:$m.NONCE_L}},exit:0},
    {a:["send","--help"],out:"Usage: glen send [OPTIONS]",exit:0},
    {a:["tail","-h"],out:"Usage: glen tail [OPTIONS]",exit:0},
    {a:["inbox","--help"],out:"Usage: glen inbox [OPTIONS]",exit:0}
  ] | to_entries[] | .key as $i | .value as $v |
  {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
  {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$S/calls.ndjson"
HOME="$S/c2-home" "$A/acc.sh" check c2 "$S"
jq -e '.mode=="c2" and .pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
# R's only send being usage-rejected cannot satisfy the c2 RPC refusal check.
jq -c 'if .t=="end" and .argv[0]=="send" and .argv[1]=="aaaaaaa" then .exit=2 | .stdout={error:{code:"usage"}} else . end' "$S/calls.ndjson" > "$S/c2-usage.ndjson"
cp "$S/c2-usage.ndjson" "$S/calls.ndjson"
if HOME="$S/c2-home" "$A/acc.sh" check c2 "$S"; then echo 'usage-only R refusal passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-418-rpc-refused" and (.pass|not)) and all(.[];.pass or .id=="AC-418-rpc-refused")' "$S/verdict.json" >/dev/null
# A genuine R refusal still satisfies c2 alongside the excluded usage rejection.
cp "$S/c2-usage.ndjson" "$S/calls.ndjson"
jq -cn '{t:"start",id:"r-refused",argv:["send","aaaaaaa","hi"],ts:100},
  {t:"end",id:"r-refused",argv:["send","aaaaaaa","hi"],exit:6,ms:20,
   stdout:{error:{details:{reason:"sandboxed",network:true}}}}' >> "$S/calls.ndjson"
HOME="$S/c2-home" "$A/acc.sh" check c2 "$S"
jq -e '.mode=="c2" and .pass and (.checks|any(.id=="AC-418-rpc-refused" and .pass) and all(.pass))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Exercise the guard with a fake binary, including --as before target and escaped cwd.
printf '#!/usr/bin/env bash\nprintf '\''{"v":1,"ok":true,"data":{}}\\n'\''\n' > "$S/glen-bin/glen"
chmod +x "$S/glen-bin/glen"
printf '#!/usr/bin/env bash\nACC_S=%q\n' "$S" > "$S/bin/glen"
tail -n +2 "$A/glen-wrapper.sh" >> "$S/bin/glen"; chmod +x "$S/bin/glen"
# A bare installed glen call must record a paired start/end, including on bash 3.2.
: > "$S/calls.ndjson"
"$S/bin/glen" >/dev/null
jq -se 'length==2 and .[0].t=="start" and .[0].argv==[] and
  .[1].t=="end" and .[1].id==.[0].id and .[1].argv==[] and .[1].exit==0 and
  .[1].stdout=={v:1,ok:true,data:{}}' "$S/calls.ndjson" >/dev/null
# c2 help calls bypass the guard but retain normal paired recordings.
: > "$S/calls.ndjson"
for help in --help -h -fh --version -V; do
    "$S/bin/glen" send "$help" >/dev/null
done
"$S/bin/glen" send --as --help >/dev/null
jq -se '. as $calls | length==12 and all(.[];.t!="guard_refused") and
  ([.[]|select(.t=="start")]|length)==6 and
  all(.[]|select(.t=="start"); . as $s | any($calls[];.t=="end" and .id==$s.id and .argv==$s.argv and .exit==0 and .stdout=={v:1,ok:true,data:{}}))' "$S/calls.ndjson" >/dev/null
"$A/acc.sh" check c2 "$S" >/dev/null || true
jq -e '.checks|any(.id=="guard_refused" and .pass)' "$S/verdict.json" >/dev/null
# Help after the separator is a target, and foreign handles remain guarded.
for target in --help -h -fh --version -V deadbeef; do
    if "$S/bin/glen" send -- "$target" hi >/dev/null 2>&1; then
        echo 'unguarded c2 target allowed' >&2; exit 1
    else
        [[ $? == 99 ]] || { echo 'unexpected c2 guard exit' >&2; exit 1; }
    fi
done
jq -se '[.[]|select(.t=="guard_refused")]|length==6' "$S/calls.ndjson" >/dev/null
"$S/bin/glen" send --as auto aaaaaaa hi >/dev/null
"$S/bin/glen" send @new hi --cwd "$S/project" >/dev/null
if "$S/bin/glen" send @new hi --cwd "$S/project/../.." >/dev/null 2>&1; then echo 'escape allowed' >&2; exit 1; fi
if "$S/bin/glen" send deadbeef hi >/dev/null 2>&1; then echo 'foreign session allowed' >&2; exit 1; fi
if "$S/bin/glen" install-extension >/dev/null 2>&1; then echo 'extension install allowed' >&2; exit 1; fi
# Mode p (v0.2 W2, AC-v2-206): one send to P, then the background reply wait and its wake class.
Q="$S/p"
mkdir -p "$Q/bin" "$Q/glen-bin"
jq -n --arg s "$Q" '{mode:"p",scratch:$s,project:($s+"/project"),P:{id:"01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",handle:"e27c0de"},NONCE_P:"P-nonce",
  env:{HOME:($s+"/omp-home"),PI_CODING_AGENT_DIR:($s+"/omp-agent"),GLEN_OMP_BIN:($s+"/fake-bin/fake_omp"),FAKE_OMP_SCENARIO:"progress",FAKE_OMP_DELAY_MS:"100000",LD_PRELOAD:"/nonexistent/acc-preload.so"},buckets:[]}' > "$Q/manifest.json"
p_calls() { # extra argv arrays (JSON) appended after the send and the reply wait
    jq -cn --slurpfile m "$Q/manifest.json" --argjson extra "${1:-[]}" '
      $m[0] as $m |
      ([ {a:["send","e27c0de","Call your reply tool with exactly P-nonce","--no-wait"],out:{data:{job:{session_id:$m.P.id,handle:"j1234567"}}},exit:10},
         {a:["wait","j1234567","--for","reply"],out:{data:{woke:"reply",replies:[{text:"reply: P-nonce"}]}},exit:10} ]
       + [$extra[] | {a:.,out:{data:{messages:[]}},exit:0}]) | to_entries[] | .key as $i | .value as $v |
      {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
      {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$Q/calls.ndjson"
}
p_events() { # $1: the reply wait's run_in_background; $2: its task b1's completion notification:
    # none, system (stream-json's task_notification event) or xml (the <task-notification> user
    # message), after which the run reads the output file; $3: a foreground retrieval of b1's
    # output before that notification: none, TaskOutput, BashOutput or Read; $4: live (the
    # notification arrives inside the turn) or resumed (the shape Claude Code 2.1.316 recorded: the
    # turn ends with a first result, then task_updated, the notification, a new init and the
    # resumed turn, which ends with the final result).
    jq -cn --argjson bg "$1" --arg note "${2:-none}" --arg fetch "${3:-none}" --arg shape "${4:-live}" '
      "{\"data\":{\"woke\":\"reply\",\"replies\":[{\"text\":\"reply: P-nonce\"}]}}" as $out |
      "Background command \"glen wait j1234567 --for reply\" failed with exit code 10" as $summary |
      {type:"system",subtype:"init"},
      {type:"assistant",message:{content:[{type:"tool_use",id:"send",name:"Bash",input:{command:"glen send e27c0de \"Call your reply tool with exactly P-nonce\" --no-wait"}}]}},
      {type:"user",message:{content:[{type:"tool_result",tool_use_id:"send",content:"{\"v\":1,\"ok\":true,\"data\":{\"status\":\"running\"}}"}]}},
      {type:"assistant",message:{content:[{type:"tool_use",id:"wait",name:"Bash",input:({command:"glen wait j1234567 --for reply"} + if $bg then {run_in_background:true} else {} end)}]}},
      {type:"user",message:{content:[{type:"tool_result",tool_use_id:"wait",content:(if $bg then "Command running in background with ID: b1. Output is being written to: /tmp/tasks/b1.output" else $out end)}]},
       tool_use_result:(if $bg then {stdout:"",stderr:"",interrupted:false,backgroundTaskId:"b1"} else {stdout:$out,stderr:"",interrupted:false} end)},
      (if $fetch == "none" then empty else
        {type:"assistant",message:{content:[{type:"tool_use",id:"fetch",name:$fetch,input:(if $fetch == "TaskOutput" then {task_id:"b1",block:true,timeout:600000} elif $fetch == "BashOutput" then {bash_id:"b1"} else {file_path:"/tmp/tasks/b1.output"} end)}]}},
        {type:"user",message:{content:[{type:"tool_result",tool_use_id:"fetch",content:"<retrieval_status>success</retrieval_status>\n<task_id>b1</task_id>\n<status>failed</status>\n<exit_code>10</exit_code>\n<output>\($out)</output>"}]}} end),
      (if $shape == "resumed" then
        {type:"assistant",message:{content:[{type:"text",text:"Waiting for the reply."}]}},
        {type:"result",result:"Waiting for the reply."},
        (if $bg and $note != "none" then {type:"system",subtype:"task_updated",task_id:"b1",patch:{status:"failed"}} else empty end)
       else empty end),
      (if ($bg|not) or $note == "none" then empty else
        (if $note == "system" then
          {type:"system",subtype:"task_notification",task_id:"b1",tool_use_id:"wait",status:"failed",output_file:"/tmp/tasks/b1.output",summary:$summary}
        else
          {type:"user",message:{role:"user",content:[{type:"text",text:"<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>wait</tool-use-id>\n<output-file>/tmp/tasks/b1.output</output-file>\n<status>failed</status>\n<summary>\($summary)</summary>\n</task-notification>"}]}}
        end),
        (if $shape == "resumed" then {type:"system",subtype:"init"} else empty end),
        {type:"assistant",message:{content:[{type:"tool_use",id:"read",name:"Read",input:{file_path:"/tmp/tasks/b1.output"}}]}},
        {type:"user",message:{content:[{type:"tool_result",tool_use_id:"read",content:$out}]}} end),
      {type:"assistant",message:{content:[{type:"text",text:"done"}]}},
      {type:"result",result:"reply: P-nonce"}' > "$Q/p.jsonl"
}
p_verdict() { # $1: expected wake class, $2: the checks expected to fail (JSON array)
    "$A/acc.sh" check p "$Q" >/dev/null || true
    jq -e --arg wake "$1" --argjson failing "$2" '
      (.checks|map(select(.pass|not).id)) == $failing and (.pass == ($failing|length == 0)) and
      (.checks|any(.id=="AC-206-evidence" and (.evidence|startswith("wake="+$wake+" "))))' "$Q/verdict.json" >/dev/null ||
      { echo "mode p: expected wake=$1 failing=$2" >&2; jq . "$Q/verdict.json" >&2; exit 1; }
}
printf 'reply: P-nonce\n' > "$Q/report.md"
p_calls; p_events true system; p_verdict woke '[]'
p_events true xml; p_verdict woke '[]'
p_events true none; p_verdict no_wake '[]'
# Only b1's own completion notification is a wake: output a foreground retrieval returned first
# (TaskOutput blocking on b1, or BashOutput), another task's notification and a stopped or killed
# b1 are no wake.
p_events true none TaskOutput; p_verdict no_wake '[]'
p_events true xml TaskOutput; p_verdict no_wake '[]'
p_events true system BashOutput; p_verdict no_wake '[]'
p_events true system
jq -c 'if .subtype?=="task_notification" then .task_id="b9" | .tool_use_id="other" else . end' "$Q/p.jsonl" > "$Q/p-other.jsonl"
mv "$Q/p-other.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
p_events true system; jq -c 'if .subtype?=="task_notification" then .status="stopped" else . end' "$Q/p.jsonl" > "$Q/p-stop.jsonl"
mv "$Q/p-stop.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
p_events true xml; jq -c 'walk(if type=="string" then sub("<status>failed</status>";"<status>killed</status>") else . end)' "$Q/p.jsonl" > "$Q/p-kill.jsonl"
mv "$Q/p-kill.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
# The recorded shape: a first result ends the turn, the process stays alive, and b1's notification
# resumes the run (an assistant tool_use after it). A result before the notification is no bar.
p_events true system none resumed; p_verdict woke '[]'
jq -e '.checks[] | select(.id=="AC-206-evidence").evidence |
  capture("delivered_event=(?<d>[0-9]+) resumed_event=(?<r>[0-9]+) first_result_event=(?<f>[0-9]+) result_event=(?<l>[0-9]+)") |
  map_values(tonumber) | .f < .d and .d < .r and .r < .l' "$Q/verdict.json" >/dev/null ||
  { echo 'mode p: resumed evidence fields out of order' >&2; jq . "$Q/verdict.json" >&2; exit 1; }
p_events true xml none resumed; p_verdict woke '[]'
# No resumed activity is no wake: the run ends at the notification, or only an init and the
# final result follow it.
p_events true system none resumed
jq -sc '(map(.subtype? == "task_notification") | index(true)) as $k | .[:$k+1][]' "$Q/p.jsonl" > "$Q/p-end.jsonl"
mv "$Q/p-end.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
for shape in live resumed; do
    p_events true system none "$shape"
    jq -sc '(map(.subtype? == "task_notification") | index(true)) as $k | to_entries[] |
      select(.key <= $k or (.value.type | IN("assistant", "user") | not)) | .value' "$Q/p.jsonl" > "$Q/p-idle.jsonl"
    mv "$Q/p-idle.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
done
# A foreground retrieval of b1 before its notification is no wake even when the output it got
# lacks the nonce (the fake's first reply is `progress`); another task's notification neither.
for fetch in TaskOutput BashOutput Read; do
    p_events true system "$fetch" resumed
    jq -c 'walk(if type=="string" then gsub("reply: P-nonce"; "progress") else . end)' "$Q/p.jsonl" > "$Q/p-progress.jsonl"
    mv "$Q/p-progress.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
    jq -e '.checks|any(.id=="AC-206-evidence" and (.evidence|contains("fetched_before_delivery=true")))' "$Q/verdict.json" >/dev/null ||
      { echo "mode p: $fetch before the notification was not a retrieval" >&2; exit 1; }
done
p_events true system none resumed
jq -c 'if .subtype?=="task_notification" then .task_id="b9" | .tool_use_id="other" else . end' "$Q/p.jsonl" > "$Q/p-other.jsonl"
mv "$Q/p-other.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
p_events false; p_verdict foreground_only '["AC-206-bg-wait-started"]'
jq -c 'select(.message.content[0].tool_use_id? != "wait" and .message.content[0].id? != "wait")' "$Q/p.jsonl" > "$Q/p-none.jsonl"
mv "$Q/p-none.jsonl" "$Q/p.jsonl"; p_verdict no_wait '["AC-206-bg-wait-started"]'
# A woken run must quote the nonce; three inbox calls, or one without --wait/--peek, is polling.
p_events true system; printf 'no nonce here\n' > "$Q/report.md"; p_verdict woke '["AC-206-nonce"]'
printf 'reply: P-nonce\n' > "$Q/report.md"
p_calls '[["inbox","--peek"],["inbox","--wait"],["inbox","--peek"]]'; p_verdict woke '["AC-206-no-inbox-polling"]'
p_calls '[["inbox"]]'; p_verdict woke '["AC-206-no-inbox-polling"]'
p_calls '[["inbox","--wait","--timeout","90s"],["inbox","--peek"]]'; p_verdict woke '[]'
# A foreground glen call that carries the nonce means the background wait did not deliver it.
jq -c 'if .type=="user" and .message.content[0].tool_use_id?=="send" then .message.content[0].content="reply: P-nonce" else . end' "$Q/p.jsonl" > "$Q/p-fg.jsonl"
mv "$Q/p-fg.jsonl" "$Q/p.jsonl"; p_verdict no_wake '[]'
# A run that never ended has no result event; a second send is not exactly one.
p_events true system; jq -c 'select(.type!="result")' "$Q/p.jsonl" > "$Q/p-open.jsonl"; mv "$Q/p-open.jsonl" "$Q/p.jsonl"
p_verdict woke '["AC-206-evidence"]'
p_events true system
jq -c 'select(.t!="end" or .argv[0]!="send"), (select(.t=="end" and .argv[0]=="send") | ., (.id="again" | .)), (select(.t=="start" and .argv[0]=="send") | .id="again")' "$Q/calls.ndjson" > "$Q/calls-two.ndjson"
mv "$Q/calls-two.ndjson" "$Q/calls.ndjson"; p_verdict woke '["AC-206-send"]'
p_calls
rm "$Q/calls.ndjson"
if "$A/acc.sh" check p "$Q" >/dev/null; then echo 'missing p recording passed' >&2; exit 1; fi
jq -e '(.pass|not) and (.checks|length)==8 and all(.checks[]; (.pass|not) and (.evidence|contains("missing recording")))' "$Q/verdict.json" >/dev/null
# The wrapper passes only whitelisted manifest env names, accepts P, records the receivers' argv
# intact, and adds --profile only for a manifest that names glen-harness.
cat > "$Q/glen-bin/glen" <<'EOF'
#!/usr/bin/env bash
jq -cn --args '{v:1,ok:true,data:{argv:$ARGS.positional,preload:($ENV.LD_PRELOAD // null),scenario:($ENV.FAKE_OMP_SCENARIO // null),home:($ENV.HOME // null),agent:($ENV.PI_CODING_AGENT_DIR // null)}}' -- "$@"
EOF
chmod +x "$Q/glen-bin/glen"
printf '#!/usr/bin/env bash\nACC_S=%q\n' "$Q" > "$Q/bin/glen"
tail -n +2 "$A/glen-wrapper.sh" >> "$Q/bin/glen"; chmod +x "$Q/bin/glen"
: > "$Q/calls.ndjson"
out=$(env -u LD_PRELOAD "$Q/bin/glen" send e27c0de hi --no-wait)
jq -e --arg q "$Q" '.data.argv==["send","e27c0de","hi","--no-wait"] and .data.preload==null and .data.scenario=="progress" and
  .data.home==($q+"/omp-home") and .data.agent==($q+"/omp-agent")' <<< "$out" >/dev/null || { echo "mode p wrapper env: $out" >&2; exit 1; }
"$Q/bin/glen" wait j1234567 --for reply --after r1 >/dev/null
"$Q/bin/glen" inbox --wait --timeout 90s >/dev/null
jq -se 'all(.[];.t!="guard_refused") and ([.[]|select(.t=="end")|.argv]==[["send","e27c0de","hi","--no-wait"],["wait","j1234567","--for","reply","--after","r1"],["inbox","--wait","--timeout","90s"]])' "$Q/calls.ndjson" >/dev/null
if "$Q/bin/glen" send deadbeef hi >/dev/null 2>&1; then echo 'foreign P-mode target allowed' >&2; exit 1; else [[ $? == 99 ]]; fi
jq '. + {profile:"glen-harness"}' "$Q/manifest.json" > "$Q/manifest.tmp"; mv "$Q/manifest.tmp" "$Q/manifest.json"
out=$("$Q/bin/glen" ls --json)
jq -e '.data.argv==["--profile","glen-harness","ls","--json"]' <<< "$out" >/dev/null || { echo "profile not prepended: $out" >&2; exit 1; }
jq -se '[.[]|select(.t=="end")]|last|.argv==["ls","--json"]' "$Q/calls.ndjson" >/dev/null
jq '.profile="default"' "$Q/manifest.json" > "$Q/manifest.tmp"; mv "$Q/manifest.tmp" "$Q/manifest.json"
if "$Q/bin/glen" ls >/dev/null 2>&1; then echo 'foreign profile allowed' >&2; exit 1; else [[ $? == 99 ]]; fi
# Mode m (v0.2 W5, AC-v2-505): the recording launcher, then the wire-log checks and teardown.
Q="$S/m"
mkdir -p "$Q/bin" "$Q/glen-bin" "$Q/project"
jq -n --arg s "$Q" '{mode:"m",scratch:$s,project:($s+"/project"),sessions_dir:($s+"/omp-agent/sessions"),
  P:{id:"01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de",handle:"e27c0de"},NONCE_M:"M-nonce",
  env:{HOME:($s+"/omp-home"),PI_CODING_AGENT_DIR:($s+"/omp-agent"),GLEN_OMP_BIN:($s+"/fake-bin/fake_omp"),GLEN_HOME:"/elsewhere",
       FAKE_OMP_SCENARIO:"progress",FAKE_OMP_DELAY_MS:"8000",LD_PRELOAD:"/nonexistent/acc-preload.so",OMP_PROFILE:"default",
       PI_CONFIG_DIR:".omp-owner",CLAUDE_CONFIG_DIR:"/nonexistent/claude",CODEX_HOME:"/nonexistent/codex"},buckets:[]}' > "$Q/manifest.json"
# A stand-in for `glen mcp`: one answer per request line, carrying argv and environment names only.
cat > "$Q/glen-bin/glen" <<'EOF'
#!/usr/bin/perl
use strict; use warnings; $| = 1;
my $argv = join ",", map { "\"$_\"" } @ARGV;
my $env = join ",", map { "\"$_\"" } sort keys %ENV;
my $id = 0;
while (<STDIN>) { print "{\"jsonrpc\":\"2.0\",\"id\":", $id++, ",\"result\":{\"argv\":[$argv],\"env\":[$env]}}\n"; }
EOF
chmod +x "$Q/glen-bin/glen"
write_mcp_launcher "$Q"
{ [[ $(head -n 1 "$Q/bin/glen-mcp") == '#!/usr/bin/env bash' ]] && grep -q 'exec env -i PATH=/usr/bin:/bin ' "$Q/bin/glen-mcp" &&
  grep -qF "GLEN_HOME='$Q/home'" "$Q/bin/glen-mcp" && ! grep -qE 'elsewhere|LD_PRELOAD|OMP_PROFILE|PI_CONFIG_DIR|CLAUDE_|CODEX_' "$Q/bin/glen-mcp"; } ||
  { echo 'mode m launcher does not close the environment' >&2; cat "$Q/bin/glen-mcp" >&2; exit 1; }
# A scripted session: the launcher ends when the client closes stdin (perl's alarm bounds a hang),
# the host's own environment does not reach the server, and the wire log holds both directions.
printf '%s\n' '{"jsonrpc":"2.0","id":0,"method":"initialize"}' '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' > "$Q/session-in.ndjson"
out=$(env OMP_PROFILE=leak CLAUDE_CONFIG_DIR=/leak LD_PRELOAD=/nonexistent/acc-preload.so perl -e 'alarm 10; exec @ARGV or exit 127' "$Q/bin/glen-mcp" < "$Q/session-in.ndjson") ||
  { echo "mode m launcher session failed: exit $?" >&2; exit 1; }
jq -se 'length == 2 and all(.[]; .result.argv == ["mcp"] and
  .result.env == ["FAKE_OMP_DELAY_MS","FAKE_OMP_SCENARIO","GLEN_HOME","GLEN_OMP_BIN","HOME","PATH","PI_CODING_AGENT_DIR"])' <<< "$out" >/dev/null ||
  { echo "mode m launcher env: $out" >&2; exit 1; }
{ cmp -s "$Q/session-in.ndjson" "$Q/mcp-in.ndjson" && [[ $(cat "$Q/mcp-out.ndjson") == "$out" ]]; } ||
  { echo 'mode m wire log differs from the session' >&2; exit 1; }
m_wire() { # $1: initialize protocolVersion ("" drops initialize), $2: glen_send calls, $3: the first send's isError, $4: wait or nowait
    jq -cn --arg v "$1" --argjson sends "$2" --argjson err "$3" --arg wait "$4" '
      def call($id; $name; $args): {jsonrpc:"2.0",id:$id,method:"tools/call",params:{name:$name,arguments:$args}};
      def answer($id; $doc; $error): {jsonrpc:"2.0",id:$id,result:{content:[{type:"text",text:($doc|tojson)}],isError:$error}};
      {v:1,ok:true,data:{job:{handle:"j1234567",id:"019a2b3c-0000-7000-8000-000000000001",session_id:"01a0f6b1-2680-7a3c-8b21-4a1f9e27c0de"}}} as $job |
      {in:[(if $v == "" then empty else {jsonrpc:"2.0",id:0,method:"initialize",params:{protocolVersion:$v,capabilities:{},clientInfo:{name:"claude-code",version:"2.1.316"}}} end),
           {jsonrpc:"2.0",method:"notifications/initialized"},
           {jsonrpc:"2.0",id:1,method:"tools/list"},
           call(2; "glen_ls"; {}),
           (range($sends) | call(3 + .; "glen_send"; {session:"e27c0de",message:"M-nonce",no_wait:true})),
           (if $wait == "wait" then call(9; "glen_wait"; {job:"j1234567"}) else empty end)],
       out:[(if $v == "" then empty else {jsonrpc:"2.0",id:0,result:{protocolVersion:"2025-06-18",capabilities:{tools:{}},serverInfo:{name:"glen",version:"0.2.0"}}} end),
            {jsonrpc:"2.0",id:1,result:{tools:[]}},
            answer(2; {v:1,ok:true,data:{sessions:[]}}; false),
            (range($sends) | if . == 0 and $err then answer(3; {v:1,ok:false,error:{code:"usage",message:"bad arguments"}}; true) else answer(3 + .; $job; false) end),
            (if $wait == "wait" then answer(9; {v:1,ok:true,data:{job:{handle:"j1234567"},replies:[{text:"reply: M-nonce"}]}}; false) else empty end)]}' > "$Q/wire.json"
    jq -c '.in[]' "$Q/wire.json" > "$Q/mcp-in.ndjson"; jq -c '.out[]' "$Q/wire.json" > "$Q/mcp-out.ndjson"
}
m_events() { # $1: one more tool_use name in m.jsonl ("" for none)
    jq -cn --arg extra "$1" '
      {type:"system",subtype:"init"},
      ((["mcp__glen__glen_ls","mcp__glen__glen_send","mcp__glen__glen_wait"] + (if $extra == "" then [] else [$extra] end))[] |
        {type:"assistant",message:{content:[{type:"tool_use",id:.,name:.,input:{}}]}}),
      {type:"result",result:"reply: M-nonce",num_turns:4,total_cost_usd:0.2}' > "$Q/m.jsonl"
}
m_verdict() { # $1: the checks expected to fail (JSON array)
    "$A/acc.sh" check m "$Q" >/dev/null || true
    jq -e --argjson failing "$1" '(.checks|length) == 7 and (.checks|map(select(.pass|not).id)) == $failing and
      (.pass == ($failing|length == 0))' "$Q/verdict.json" >/dev/null ||
      { echo "mode m: expected failing=$1" >&2; jq . "$Q/verdict.json" >&2; exit 1; }
}
: > "$Q/calls.ndjson"; printf 'reply: M-nonce\n' > "$Q/report.md"
m_wire 2025-06-18 1 false wait; m_events ''; m_verdict '[]'
jq -e '.checks[] | select(.id=="AC-505-initialize").evidence == "client=claude-code/2.1.316 protocol=2025-06-18 first_method=initialize"' "$Q/verdict.json" >/dev/null ||
  { echo 'mode m: AC-505-initialize evidence' >&2; jq . "$Q/verdict.json" >&2; exit 1; }
m_wire 2025-11-25 1 false wait; m_verdict '[]'
m_events ToolSearch; m_verdict '[]'
m_events Bash; m_verdict '["AC-505-no-shell"]'
m_events mcp__glen__glen_inbox; m_verdict '["AC-505-no-shell"]'
# The Codex leg leaves no m.jsonl: the wire log alone decides.
rm "$Q/m.jsonl"; m_verdict '[]'
m_events ''
m_wire 2025-06-18 1 false nowait; m_verdict '["AC-505-wait"]'
m_wire 2025-06-18 2 false wait; m_verdict '["AC-505-send"]'
m_wire 2025-06-18 1 true wait; m_verdict '["AC-505-send","AC-505-wait"]'
m_wire 2026-07-28 1 false wait; m_verdict '["AC-505-initialize"]'
m_wire '' 1 false wait; m_verdict '["AC-505-initialize"]'
m_wire 2025-06-18 1 false wait; printf 'no nonce here\n' > "$Q/report.md"; m_verdict '["AC-505-nonce"]'
printf 'reply: M-nonce\n' > "$Q/report.md"
rm "$Q/calls.ndjson"
if "$A/acc.sh" check m "$Q" >/dev/null; then echo 'missing m recording passed' >&2; exit 1; fi
jq -e '(.pass|not) and (.checks|length)==7 and all(.checks[]; (.pass|not) and (.evidence|contains("missing recording")))' "$Q/verdict.json" >/dev/null
# Teardown of a passing mode m run exits 0: no owner-session snapshot, the wire log in evidence.
: > "$Q/calls.ndjson"; m_verdict '[]'
if ! (unset CLAUDE_CONFIG_DIR; HOME="$Q/owner-home"
      for dir in "$HOME/.claude/skills" "$HOME/.agents/skills"; do ls -lad "$dir" "$dir/glen" "$dir/glen/SKILL.md" 2>/dev/null || true; done > "$Q/owner-skills.before.txt"
      teardown "$Q" "$Q/evidence") > "$Q/teardown.log" 2>&1; then
    echo 'mode m teardown failed' >&2; cat "$Q/teardown.log" >&2; exit 1
fi
for file in m.jsonl mcp-in.ndjson mcp-out.ndjson verdict.json manifest.json; do [[ -f $Q/evidence/$file ]] || { echo "mode m evidence lacks $file" >&2; exit 1; }; done
[[ ! -e $Q/owner-sessions.after.json && ! -e $Q/evidence/sessions-left.txt ]] || { echo 'mode m took an owner-session snapshot' >&2; exit 1; }
jq -e '.pass and (.checks|any(.id=="AC-419-teardown" and .pass))' "$Q/evidence/verdict.json" >/dev/null
# Fake token is assembled at runtime so the repository does not itself contain a credential-shaped string.
mkdir "$S/leak"
printf 'token = "sk-ant-%s-%s"\n' api03 "$(printf 'a1B2c3D4e5F6g7H8i9J0%.0s' {1..5} | head -c 93)AA" > "$S/leak/fake.txt"
if gitleaks dir --no-banner --redact "$S/leak" >/dev/null 2>&1; then echo 'fake token was not detected' >&2; exit 1; fi
printf 'offline harness fixture checks passed\n'
