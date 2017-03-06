#!/bin/bash

PLATFORM=linux-x86_64
VERSION=v0.1.2

sudo apt-get install gcc
rm -f grcov grcov-$PLATFORM.tar.bz2
wget https://github.com/marco-c/grcov/releases/download/$VERSION/grcov-$PLATFORM.tar.bz2
tar xf grcov-$PLATFORM.tar.bz2
rm grcov-$PLATFORM.tar.bz2
