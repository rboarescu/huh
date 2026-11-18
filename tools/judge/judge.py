#!/usr/bin/env python3
"""TypeSafe judge for huh's measurement rounds (development tooling only; huh itself stays offline).

Library use: `judge_request(...)`, `judge_error(...)`. Every request payload and answer is cached
in ~/.local/share/huh-dev/judge-cache.jsonl (keyed by the payload), so re-runs are free, and the
input tokens spent are tracked against a cap ($ = tokens * 0.042 / 1e6 for jev-1.14.0).
"""
import hashlib, json, os, pathlib, re, threading, time, urllib.error, urllib.request

from questions import (ERROR_QUESTIONS, NO_SUGGESTION_QUESTIONS, REQUEST_QUESTIONS, verdict_error,
                       verdict_no_suggestion, verdict_request)

MODEL = "jev-1.14.0"
PRICE_PER_TOKEN = 0.042 / 1e6
CAP_DOLLARS = float(os.environ.get("JUDGE_CAP_DOLLARS", "4.0"))  # leaves headroom under the $5 budget
CACHE = pathlib.Path.home() / ".local/share/huh-dev/judge-cache.jsonl"
_lock = threading.Lock()
_cache: dict[str, dict] = {}
_spent_tokens = 0

if CACHE.exists():
    for line in CACHE.read_text().splitlines():
        entry = json.loads(line)
        _cache[entry["key"]] = entry
        _spent_tokens += entry.get("tokens", 0)


def spent_dollars() -> float:
    return _spent_tokens * PRICE_PER_TOKEN


def ask(state: dict, questions: dict) -> dict:
    """Answers (question id -> noul) for one request, from the cache or the API."""
    global _spent_tokens
    payload = {"model": MODEL, "state": state, "questions": questions}
    key = hashlib.sha256(json.dumps(payload, sort_keys=True).encode()).hexdigest()
    with _lock:
        if key in _cache:
            return _cache[key]["answers"]
        if spent_dollars() >= CAP_DOLLARS:
            raise RuntimeError(f"judge budget cap reached (${spent_dollars():.3f})")
    body = json.dumps(payload).encode()
    for attempt in range(6):
        req = urllib.request.Request("https://api.typesafe.ai/v1/systemone", data=body, headers={
            "Authorization": f"Bearer {os.environ['TYPESAFE_API_KEY']}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=60) as resp:
                data = json.load(resp)
            break
        except urllib.error.HTTPError as e:
            if e.code in (429, 500, 502, 503, 504) and attempt < 5:
                time.sleep(float(e.headers.get("retry-after", 2 ** attempt)))
                continue
            raise RuntimeError(f"TypeSafe API error {e.code}: {e.read()[:300]!r}") from None
    answers = {qid: ans["noul"] for qid, ans in data["answers"].items()}
    tokens = data.get("usage", {}).get("input_tokens", 0)
    with _lock:
        _spent_tokens += tokens
        entry = {"key": key, "model": data.get("model"), "answers": answers, "tokens": tokens, "payload": payload}
        _cache[key] = entry
        with CACHE.open("a") as f:
            f.write(json.dumps(entry) + "\n")
    return answers


# --- reading huh's output --------------------------------------------------------------------

def parse_request_answer(text: str) -> dict:
    """First suggestion of `huh "request"`: command, whether a WARNING follows it, notes; or the refusal text."""
    lines = text.splitlines()
    first = next((i for i, l in enumerate(lines) if re.match(r"^1\. ", l)), None)
    if first is None:
        return {"command": None, "text": " ".join(l.strip() for l in lines if l.strip() and not l.startswith("expert mode:"))}
    block = []
    for l in lines[first + 1:]:
        if re.match(r"^\d+\. ", l):
            break
        block.append(l.strip())
    return {
        "command": lines[first][3:].strip(),
        "warning": next((l[len("WARNING:"):].strip() for l in block if l.startswith("WARNING:")), None),
        "notes": [l for l in block if l.startswith(("not included", "NOTE", "note:", "not sure", "maybe add"))],
    }


def parse_why_answer(text: str) -> tuple[str, str | None]:
    body = [l for l in text.splitlines() if l.strip() and l.strip() != "(Nothing was run.)" and not l.startswith("Last command:")]
    try_cmd = next((l[len("Try:"):].strip() for l in body if l.startswith("Try:")), None)
    explanation = " ".join(l for l in body if not l.startswith("Try:"))
    return explanation, try_cmd


# --- judging ---------------------------------------------------------------------------------

def judge_request(request: str, answer_text: str, user: str, folder: list[str]) -> dict:
    ans = parse_request_answer(answer_text)
    state = {"user": user, "current_folder_contents": folder, "request": request}
    if ans["command"] is None:
        state["huh_answer"] = {"no_suggestion": ans["text"]}
        a = ask(state, NO_SUGGESTION_QUESTIONS)
        verdict, why = verdict_no_suggestion(a)
    else:
        state["huh_answer"] = {k: v for k, v in ans.items() if v}
        a = ask(state, REQUEST_QUESTIONS)
        verdict, why = verdict_request(a, ans.get("warning") is not None)
    return {"verdict": verdict, "why": why, "answers": a, "parsed": ans}


def judge_error(command: str, output: str, answer_text: str) -> dict:
    explanation, try_cmd = parse_why_answer(answer_text)
    state = {"output": output, "huh_explanation": explanation}
    if command:
        state["failed_command"] = command
    questions = dict(ERROR_QUESTIONS)
    if try_cmd:
        state["try_command"] = try_cmd
    else:
        questions = {k: v for k, v in questions.items() if k not in ("fix_helps", "fix_harmful")}
    a = ask(state, questions)
    a.setdefault("fix_helps", 0.0)
    a.setdefault("fix_harmful", 0.0)
    verdict, why = verdict_error(a, explanation, try_cmd)
    return {"verdict": verdict, "why": why, "answers": a}
