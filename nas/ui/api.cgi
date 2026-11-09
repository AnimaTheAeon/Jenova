#!/bin/sh
# Forwards requests from the Jenova window to the Jenova service.
exec /var/packages/Jenova/target/bin/jenova-nas cgi
