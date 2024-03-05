#!/usr/bin/bash

rurl="$1" localdir="$2" && shift 2

mkdir -p "$localdir"
cd "$localdir"

git init
git remote add -f origin "$rurl"

git config core.sparseCheckout true

# Loops over remaining args
for i; do
echo "$i" >> .git/info/sparse-checkout
done

git pull --depth 1 origin master
