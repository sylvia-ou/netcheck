# Netcheck
This is a fork of the gping project (https://github.com/orf/gping)

The key difference is that it computes the first 3 responding hops.  This makes it easier for normal people to troubleshoot their network connection and figure out if the problem is with WiFi or the Internet provider.  Gping requires the person to figure out how to use traceroute and type in the host IPs.

This also logs the raw ping time for the duration of the run and outputs a new CSV file in the same location as the binary.

![netcheck_img](https://user-images.githubusercontent.com/78395223/129528473-be562495-85b8-440c-9a55-e87a3bde3288.png)

# Releases
Currently, Windows and Linux have usable versions.
