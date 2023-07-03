#
# Messy POC by Joel Odom
#

import subprocess
from flask import Flask, render_template, request

with open('secret', 'r') as f: # Such hacked code!
    SECRET = f.read().strip()

print(f'Secret: {SECRET}.')

app = Flask(__name__)

@app.route('/', methods=['GET', 'POST'])
def home():
    if request.method == 'POST':
        text = request.form['text']
        print(f'Received request: {text}')

        if SECRET in text:
            # Write the text to a file
            with open('description.txt', 'w') as file:
                file.write(text.replace(SECRET, ''))
            # Call the external process
            result = subprocess.run('./shuffle.sh', shell=True)
        else:
            print('Secret not found.')

    return render_template('form.html')

if __name__ == '__main__':
    app.run(port=8080)
