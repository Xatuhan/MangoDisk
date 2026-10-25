on run argv
  if (count of argv) is not 5 then error number 64

  set helperPath to item 1 of argv
  set requestPath to item 2 of argv
  set responsePath to item 3 of argv
  set promptText to item 4 of argv
  set requestDigest to item 5 of argv
  set helperCommand to quoted form of helperPath & " --mangodisk-startup-helper-v5 " & quoted form of requestPath & " " & quoted form of responsePath & " " & quoted form of requestDigest

  if promptText is "" then
    do shell script helperCommand with administrator privileges
  else
    do shell script helperCommand with prompt promptText with administrator privileges
  end if
end run
