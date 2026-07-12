# d9cker demo — rendered with charmbracelet/vhs:  vhs demo/demo.tape
Output demo/d9cker.gif

Set Shell "bash"
Set FontSize 14
Set Width 1300
Set Height 470
Set Theme "Catppuccin Mocha"
Set TypingSpeed 45ms
Set Padding 12
Set BorderRadius 8

Hide
Type "export DOCKER_CONFIG=/tmp/d9cker-demo/dockercfg; export PS1='$ '; clear"
Enter
Show

Type "d9cker"
Enter
Sleep 3.5s

# running-only by default; `a` reveals exited containers (dimmed)
Type "a"
Sleep 2.5s
Type "a"
Sleep 1.5s

# filter, then Enter to stream logs live
Type "/worker"
Sleep 800ms
Enter
Sleep 1.5s
Enter
Sleep 5s
Type "q"
Sleep 800ms
Escape
Sleep 800ms

# live stats — CPU / MEM / NET / BLK, streaming
Type "/cruncher"
Sleep 800ms
Enter
Sleep 1.2s
Type "t"
Sleep 5s
Type "q"
Sleep 600ms
Escape
Sleep 600ms

# Compose tab: projects + their config file paths
Type "l"
Sleep 3s
Type "i"
Sleep 3.5s
Type "q"
Sleep 800ms

# networks, then contexts (ssh:// engines — the headline feature)
Type "5"
Sleep 2.2s
Type "6"
Sleep 3s

# global help
Type "?"
Sleep 3.5s
Escape
Sleep 500ms
Type "q"
Sleep 1s
