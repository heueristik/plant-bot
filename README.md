# plant-bot

Copy the service to the systemd directory:

```sh
sudo cp plant-bot.service /etc/systemd/system/
```

and set the `FRITZ_USERNAME` and `FRITZ_PASSWORD` environment variables inside.

Enable the service:

```sh
sudo systemctl daemon-reload
sudo systemctl enable plant-bot.service
```

Append a cronjob, e.g., to start the service at 8pm:

```sh
(crontab -l 2>/dev/null; echo "0 20 * * * sudo systemctl start <your_binary_name>.service") | crontab -
```

and monitor it with:

```sh
journalctl -u plant-bot.service -b -e -f
```

To alter it, open the crontab editor:

```sh
crontab -e
```
