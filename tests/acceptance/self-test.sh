#!/usr/bin/env bash
# Offline verification: recordings and fake executables only. Never runs setup/person/teardown.
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
jq -c 'select(.argv[0]=="send" and .argv[1]=="ccccccc")' "$S/good.ndjson" >> "$S/calls.ndjson"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="AC-415-X" and .pass))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Raw inbox NDJSON strings still supply the talk-back nonce.
jq -c 'if .t=="end" and .argv[0]=="inbox" then .stdout="{\"text\":\"T-nonce\"}\n{\"text\":\"another reply\"}\n" else . end' "$S/good.ndjson" > "$S/calls.ndjson"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="nonce_T" and .pass))' "$S/verdict.json" >/dev/null
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
# Fake token is assembled at runtime so the repository does not itself contain a credential-shaped string.
mkdir "$S/leak"
printf 'token = "sk-ant-%s-%s"\n' api03 "$(printf 'a1B2c3D4e5F6g7H8i9J0%.0s' {1..5} | head -c 93)AA" > "$S/leak/fake.txt"
if gitleaks dir --no-banner --redact "$S/leak" >/dev/null 2>&1; then echo 'fake token was not detected' >&2; exit 1; fi
printf 'offline harness fixture checks passed\n'
