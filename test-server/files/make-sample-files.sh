#!/bin/bash
# Creates the sample files in the home folder of user pi (run once while building the image).
set -e
cd ~
mkdir -p Documents Pictures Downloads test-files/nested/deeper

printf 'Hello from the OpenVNC test Pi.\nThis is a plain text file for transfer tests.\n' > Documents/hello.txt
printf 'Shopping list\n- milk\n- bread\n- Raspberry Pi 5\n' > Documents/shopping-list.txt
printf 'Type here to test the keyboard and the clipboard.\n\nÅ ä ö å € "quotes" and — dashes.\n' > Documents/notes.txt
printf 'name,value\nalpha,1\nbeta,2\ngamma,3\n' > Documents/table.csv

cp /usr/share/testpi/wallpaper.png Pictures/wallpaper.png
magick /usr/share/testpi/wallpaper.png -quality 85 Pictures/photo.jpg

head -c 1048576 /dev/urandom > test-files/random-1mb.bin
head -c 5242880 /dev/urandom > test-files/random-5mb.bin
printf 'second level\n' > test-files/nested/level2.txt
printf 'third level\n' > test-files/nested/deeper/level3.txt
printf 'Unicode file name test\n' > 'test-files/blåbär €.txt'
printf 'file with spaces\n' > 'test-files/file with spaces.txt'
