# Inputs are recordings only. This checker never invokes an agent or glen.
def check($id;$pass;$evidence): {id:$id,pass:($pass == true),evidence:$evidence};
def cmd: [.argv[]? | select(. == "ls" or . == "show" or . == "tail" or . == "send" or . == "wait" or . == "jobs" or . == "inbox" or . == "install-extension")] | first;
def first_positional:
  if length==0 then ""
  elif .[0]=="--" then .[1] // ""
  elif (.[0] | IN("--as","--wait","--cwd","--timeout","--instructions","--provider","--profile","--color")) then .[2:]|first_positional
  elif (.[0]|startswith("-")) then .[1:]|first_positional
  else .[0] end;
def target:
  .argv as $a | ([$a|to_entries[]|select(.value=="send")|.key]|first) as $i |
  if $i == null then "" else $a[$i+1:] | first_positional end;
def norm: ascii_downcase | gsub("-"; "");
def is_target($m;$k):
  (target|norm) as $t | ($m[$k].id|norm) as $id |
  ($t != "" and ($id|endswith($t))) or .stdout.data.job.session_id == $m[$k].id;
def texts: [.stdout.data.assistant_text // "", (.stdout.data.replies[]?.text // "")] | join("\n");
def nonce($n): texts | contains($n);
def final($ends):
  . as $call | if .exit == 10 or .exit == 130 then
    [$ends[] | select(cmd == "wait" and .stdout.data.job.handle == $call.stdout.data.job.handle and (.exit==0 or .exit==11))] | last // $call
  else . end;
def denied: (tostring|ascii_downcase|test("permission[^\n]*den|denied|not allowed|dontask"));
def uses($events): [$events[] | .. | objects | select(.type? == "tool_use")];
def results($events): [$events[] | .. | objects | select(.type? == "tool_result")];
def probe_success:
  . as $raw |
  (try (.content | if type=="string" then fromjson
    elif type=="array" then [.[] | select(.type?=="text") | .text? | strings] | join("\n") | fromjson
    else null end) catch null) as $parsed |
  if $parsed == null then $raw | tostring | test("\\\"?ok\\\"?\\s*:\\s*true")
  elif ($parsed|type)=="object" then $parsed.ok == true
  else false end;
$m[0] as $manifest | $calls as $all |
[$all[]|select(.t=="end")] as $ends |
[$all[]|select(.t=="start")] as $starts |
[$ends[]|select(cmd=="send")] as $sends |
[$sends[]|select(is_target($manifest;"R"))] as $r |
[$sends[]|select(is_target($manifest;"L"))] as $l |
[$sends[]|select(is_target($manifest;"X"))] as $x |
(if $mode == "b2" then
  uses($probe) as $pu | results($probe) as $pr | uses($control) as $cu | results($control) as $cr |
  [$pu[]|select(.name=="Bash" and ((.input.command // "")|if type=="string" then startswith("glen ") else false end))] as $pc |
  [$cu[]|select(.name=="Bash" and (.input.command|startswith("glen ")))] as $cc |
  [check("AC-416-probe"; any($pc[]; .id as $id | any($pr[]; .tool_use_id==$id and (denied|not) and probe_success)); "glen Bash result must succeed"),
   check("AC-416-control-denied"; any($cc[]; .id as $id | any($cr[]; .tool_use_id==$id and denied)); "otherwise inconclusive; never pass")]
 elif $mode == "a" then
  [check("AC-414-person"; $person.pass == true; $person)]
 else
 [check("paired_calls"; (all($starts[]; . as $s | any($ends[]; .id==$s.id and .argv==$s.argv))) and (all($ends[]; . as $e | any($starts[]; .id==$e.id and .argv==$e.argv))) and ([$starts[].id]|unique|length)==($starts|length) and ([$ends[].id]|unique|length)==($ends|length); "unpaired start means killed call"),
  check("guard_refused"; all($all[];.t!="guard_refused"); "zero guard refusals"),
  check("call_budget"; ($starts|length)<=40 and ($starts|length)>0 and (([$starts[] | . as $s | $ends[] | select(.id==$s.id) | $s.ts+.ms]|max)-([$starts[].ts]|min))<=1200000; "at most 40 calls within 20 minutes"),
  check("list_and_read"; any($ends[];cmd=="ls" and .exit==0) and any($ends[];(cmd=="show" or cmd=="tail") and .exit==0); "list plus transcript read"),
  check("install_extension"; all($ends[];cmd!="install-extension"); "zero extension install calls")]
 + (if $mode == "c1" then
    [check("AC-417-sandbox"; ($sends|length)>0 and all($sends[];.exit==6 and .ms<=5000 and .stdout.error.details.reason=="sandboxed" and .stdout.error.details.network==false); "every send refuses within five seconds"),
     check("AC-417-no-jobs"; $jobs_unchanged; "job directory listing unchanged"),
     check("AC-417-skill-path"; ($report|contains($scratch+"/project/.agents/skills/glen/SKILL.md")); "report gives discovered Codex skill path"),
     check("AC-417-hint"; any($sends[];.stdout.error.hint as $hint | $hint != null and ($report|contains($hint))); "report quotes sandbox hint")]
   elif $mode == "c2" then
    [check("AC-418-live"; any($l[];final($ends)|.exit==0 and .stdout.data.job.route=="live" and nonce($manifest.NONCE_L)); "live reply contains nonce"),
     check("AC-418-rpc-refused"; ($r|length)>0 and all($r[];.exit==6 and .stdout.error.details.reason=="sandboxed" and .stdout.error.details.network==true); "RPC refuses with network:true"),
     check("AC-418-config-restored"; $config_restored; "owner provides byte-compare proof after restore")]
   else
    [check("AC-415-list-RLX"; any($ends[];cmd=="ls" and .exit==0 and ([.stdout.data.sessions[].id] as $ids | all([$manifest.R.id,$manifest.L.id,$manifest.X.id][];. as $id|$ids|index($id)!=null))); "listing contains all three sessions"),
     check("AC-415-read-R"; any($ends[];(cmd=="show" or cmd=="tail") and .exit==0 and .stdout.data.session.id==$manifest.R.id); "read R"),
     check("nonce_R"; any($r[];final($ends)|.exit==0 and .stdout.data.job.route=="rpc" and nonce($manifest.NONCE_R)); "RPC final answer contains NONCE_R"),
     check("nonce_L"; any($l[];final($ends)|.exit==0 and .stdout.data.job.route=="live" and nonce($manifest.NONCE_L)); "live reply contains NONCE_L"),
     check("nonce_T"; any($ends[];cmd=="inbox" and .exit==0 and (.stdout|tostring|contains($manifest.NONCE_T))); "inbox contains NONCE_T"),
     check("AC-415-X"; ($x|length)>=1 and ($x|length)<=2 and all($x[];.exit==6); "one or two refused X sends"),
     check("AC-415-report"; all([$manifest.NONCE_R,$manifest.NONCE_L,$manifest.NONCE_T][];. as $n|$report|contains($n)) and any($x[];.stdout.error.hint as $hint|$hint!=null and ($report|contains($hint))); "report quotes nonces and X hint")]
   end)
 + (if $mode=="b" then
     uses($transcript) as $tools |
     [check("AC-415-transcript"; ($tools|length)>0 and all($tools[];.name=="Bash" or (.name=="Read" and (.input.file_path|startswith($scratch+"/")))); "required transcript: only Bash and scratch Read"),
      check("AC-415-isolation"; all($tools[]; if .name=="Bash" then (.input.command as $c |
         (($c|startswith($scratch+"/bin/glen ")) or ($c|startswith("'"+$scratch+"/bin/glen' ")) or ($c|startswith("\""+$scratch+"/bin/glen\" "))) and
         (["&&","||",";","\n","$(","`","|"] | all(.[];. as $token|$c|contains($token)|not))) else true end); "only plain scratch-wrapper calls; orchestrator also audits transcript")]
    elif $mode=="b3" then
     uses($discovered) as $tools | results($discovered) as $results |
     [check("AC-420-loaded"; any($tools[];(.name=="Skill" and ((.input.skill // .input.name // "")|test("(^|:)glen$"))) or (.name=="Read" and (.input.file_path|endswith("/skills/glen/SKILL.md")))); "discovered skill must load"),
      check("AC-420-permission"; all($tools[];if .name=="Bash" and (.input.command|test("(^|[ /])glen ")) then .id as $id | all($results[];if .tool_use_id==$id then denied|not else true end) else true end); "any denied glen call, before or after load, fails")]
    else [] end)
 end) as $checks |
{mode:$mode,pass:all($checks[];.pass),checks:$checks}
