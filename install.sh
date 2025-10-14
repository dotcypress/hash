#!/bin/sh

echo 'ACTION=="add", KERNEL=="sd[a-z][0-9]", RUN+="/usr/bin/systemd-mount --no-block --automount=yes --collect $devnode /media/hash"' > /etc/udev/rules.d/99-hash-aoutomount.rules
udevadm control --reload-rules
cp -f hash /usr/local/bin/
chmod +x /usr/local/bin/hash
cp -f hash.service /etc/systemd/system/
systemctl enable hash
systemctl start hash
echo "Hash installed"
