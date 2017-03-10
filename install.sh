#!/bin/bash

PLATFORM=linux-x86_64
LATEST_VERSION=`curl -s 'https://api.github.com/repos/marco-c/grcov/releases/latest' | python -c "import sys, json; print json.load(sys.stdin)['tag_name']"`

sudo apt-get install gcc
rm -f grcov grcov-$PLATFORM.tar.bz2
wget https://github.com/marco-c/grcov/releases/download/$LATEST_VERSION/grcov-$PLATFORM.tar.bz2
tar xf grcov-$PLATFORM.tar.bz2
rm grcov-$PLATFORM.tar.bz2

if [ -n "$1" ]; then
    sudo mv grcov $1/grcov
fi
