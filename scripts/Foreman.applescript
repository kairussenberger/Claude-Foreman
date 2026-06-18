-- Desktop launcher for Foreman: rebuilds-if-needed, then opens the app.
-- Compiled into "Foreman Launcher.app" by scripts/install-launcher.sh.
with timeout of 1800 seconds
	try
		display notification "Checking for updates…" with title "Foreman"
		do shell script "/bin/zsh " & quoted form of "/Users/kairussenberger/Developer/foreman/scripts/launch.sh"
	on error errMsg number errNum
		display dialog "Foreman couldn't build:" & return & return & errMsg buttons {"OK"} default button "OK" with icon stop with title "Foreman"
	end try
end timeout
