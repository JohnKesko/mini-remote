# OpenVNC test server: imitates a Raspberry Pi running the newer Raspberry Pi OS
# desktop (Debian trixie + labwc + wayvnc). arm64 on Apple Silicon.
# Test values only: user "pi", password "openvnc-test".
FROM debian:trixie-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        labwc wayvnc foot mousepad swaybg wlr-randr \
        tigervnc-standalone-server tigervnc-tools openbox feh xterm xfonts-base \
        openssh-server openssh-sftp-server \
        dbus dbus-bin procps openssl imagemagick \
        fonts-dejavu-core adwaita-icon-theme xkb-data \
        libpam-modules libpam-modules-bin libpam-runtime \
    && rm -rf /var/lib/apt/lists/*

# The Linux user a real Pi would have, with the test password.
RUN useradd -m -u 1000 -s /bin/bash -G video,audio pi \
    && echo 'pi:openvnc-test' | chpasswd

# Files for the server side (configs, scripts, wallpaper).
COPY files/ /opt/testpi/
RUN chmod +x /opt/testpi/*.sh

# Colourful, photo-like wallpaper (smooth gradients plus fine noise: good for JPEG tests).
RUN mkdir -p /usr/share/testpi \
    && magick -seed 7 -size 1920x1080 plasma:fractal -blur 0x1.5 \
         +noise Gaussian -attenuate 0.35 -blur 0x0.6 /usr/share/testpi/wallpaper.png

# NOTE: wayvnc/neatvnc 0.9.1 cannot offer classic VNC password login (security type 2).
# So port 5901 is TigerVNC's Xvnc (VNC password) with a small X11 desktop instead.

# Self-signed TLS certificate + TLS key, and a separate RSA key for RSA-AES login.
# (RSA key is written in the old PKCS#1 format that neatvnc reads.)
RUN mkdir -p /etc/testpi \
    && openssl req -x509 -newkey rsa:3072 -nodes -days 3650 \
         -subj "/CN=openvnc-testpi" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" \
         -keyout /etc/testpi/tls_key.pem -out /etc/testpi/tls_cert.pem \
    && openssl genrsa -traditional -out /etc/testpi/rsa_key.pem 3072 \
    && cp /opt/testpi/wayvnc-*.conf /etc/testpi/ \
    && printf 'openvnc-test\n' | vncpasswd -f > /etc/testpi/vncpasswd \
    && cp /opt/testpi/pam-wayvnc /etc/pam.d/wayvnc \
    && chown -R pi:pi /etc/testpi && chmod 600 /etc/testpi/*

# Home folder with sample files and a folder, for file-transfer tests.
RUN su pi -c /opt/testpi/make-sample-files.sh \
    && mkdir -p /home/pi/.config/labwc /home/pi/.config/foot \
    && cp /opt/testpi/labwc-autostart /home/pi/.config/labwc/autostart \
    && cp /opt/testpi/labwc-rc.xml /home/pi/.config/labwc/rc.xml \
    && cp /opt/testpi/labwc-environment /home/pi/.config/labwc/environment \
    && cp /opt/testpi/foot.ini /home/pi/.config/foot/foot.ini \
    && chown -R pi:pi /home/pi/.config

# OpenSSH: password login on, SFTP on. (Also the folder Xvnc keeps its X11 socket in.)
RUN mkdir -p /run/sshd && mkdir -m 1777 /tmp/.X11-unix \
    && cp /opt/testpi/sshd_config /etc/ssh/sshd_config.d/testpi.conf

EXPOSE 22 5900 5901 5902
ENTRYPOINT ["/opt/testpi/entrypoint.sh"]
