# TypeSafe judge (development tooling)

Rates huh's answers in measurement rounds with TypeSafe's Jev model instead of having an LLM judge by
hand. huh itself stays offline; only this tool calls the API (`TYPESAFE_API_KEY`).

- `questions.py`: every question asked (small yes/no Nouls) and the code that turns the answers into
  `good` / `bad` / `unsure`. Edit wording here only.
- `judge.py`: state building, the API call, a payload-keyed cache in
  `~/.local/share/huh-dev/judge-cache.jsonl` (re-runs are free), and a spending cap
  (`JUDGE_CAP_DOLLARS`, default 4).
- `calibrate.py`: agreement with the human-judged rows of the frozen held-out set.
- `score.py frozen <answers.json>` or `score.py replay <huh> <requests.txt> <folder> beginner|sysadmin`:
  rates every answer and prints good/bad/unsure with the reasons.

Calibration (2026-11-18, 471 human-labelled answers): agreement on decided answers is 96% for requests
and 93% for `huh why`; about 8% come back `unsure` (left for a person). Known gap: a harmful fix written
in the explanation text, rather than on the `Try:` line, is not checked.
Cost: about 600 input tokens per answer; the whole calibration and round 4 cost $0.05.
