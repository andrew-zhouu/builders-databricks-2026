#!/usr/bin/env bash
# Peek at a generated dataset and the published dev answers.
D=/home/andre/builders/databricks/data/${1:-dev}
A=/home/andre/builders/databricks/official/dev-answer/x/dev-answer
echo "--- events sample"; head -8 "$D/events/part-00000.csv"
echo "--- clicks sample"; head -6 "$D/clicks/part-00001.csv"
echo "--- links sample"; head -6 "$D/links/part-00000.csv"
echo "--- products"; head -3 "$D/products.csv"; wc -l < "$D/products.csv"
echo "--- campaigns"; head -3 "$D/campaigns.csv"; wc -l < "$D/campaigns.csv"
echo "--- file sizes (events)"; ls -lS "$D/events" | awk '{print $5}' | head -40 | tr '\n' ' '; echo
echo "--- file sizes (clicks)"; ls -lS "$D/clicks" | awk '{print $5}' | tr '\n' ' '; echo
echo "--- file sizes (links)"; ls -lS "$D/links" | awk '{print $5}' | tr '\n' ' '; echo
echo "--- line counts"; cat "$D"/events/*.csv | wc -l; cat "$D"/clicks/*.csv | wc -l; cat "$D"/links/*.csv | wc -l
echo "--- answers"; head -5 "$A/user_daily.csv"; wc -l < "$A/user_daily.csv"; head -5 "$A/campaign_daily.csv"; wc -l < "$A/campaign_daily.csv"
echo "--- id length histogram (events col1, unquoted)"
cat "$D"/events/*.csv | awk -F, '{gsub(/"/,"",$1); n[length($1)]++} END{for(k in n) printf "%s:%d ", k, n[k]; print ""}'
echo "--- user id length histogram"
cat "$D"/events/*.csv | awk -F, '{gsub(/"/,"",$2); n[length($2)]++} END{for(k in n) printf "%s:%d ", k, n[k]; print ""}'
echo "--- ts length histogram"
cat "$D"/events/*.csv | awk -F, '{gsub(/"/,"",$3); n[length($3)]++} END{for(k in n) printf "%s:%d ", k, n[k]; print ""}'
echo "--- avg line length"; cat "$D"/events/*.csv | awk '{s+=length($0)+1} END{print s/NR}'
