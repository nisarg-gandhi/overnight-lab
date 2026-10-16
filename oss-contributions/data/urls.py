import urllib.parse, sys
LBL='"good first issue","good-first-issue","help wanted","easy","Easy to Fix","sprintable","Good Second Issue","contributions welcome","beginner","first-timers-only"'
def u(q, extra=""):
    return "https://api.github.com/search/issues?q=" + urllib.parse.quote(q, safe=':/"') + extra
for line in open("repos.txt"):
    r=line.split(",")[0]
    print(r)
    print(" GFI ", u(f'repo:{r} is:issue is:open no:assignee label:{LBL}', "&sort=created&order=desc&per_page=12"))
    print(" PRM ", u(f'repo:{r} is:pr is:merged created:2026-04-16..2026-08-16', "&per_page=1"))
    print(" PRT ", u(f'repo:{r} is:pr created:2026-04-16..2026-08-16', "&per_page=1"))
    print(" IS0 ", u(f'repo:{r} is:issue comments:0 created:2026-08-01..2026-09-30', "&per_page=1"))
    print(" IST ", u(f'repo:{r} is:issue created:2026-08-01..2026-09-30', "&per_page=1"))
