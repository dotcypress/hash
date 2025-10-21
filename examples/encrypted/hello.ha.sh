#!/bin/sh

echo "Hello $(whoami)!" >> log.txt
echo "Host name: $HASH_HOST" >> log.txt
echo "Script file: $HASH_SCRIPT" >> log.txt
echo "Run directory: $(pwd)" >> log.txt

ifconfig -a | openssl aes-256-cbc -e -pbkdf2 -pass file:/etc/hash/private_key.pem > ifconfig.enc
