#
# Messy POC by Joel Odom
#

from flask import Flask, render_template, request

app = Flask(__name__)

@app.route('/', methods=['GET', 'POST'])
def home():
    if request.method == 'POST':
        text = request.form['text']
        return f'Text entered: {text}'
    return render_template('form.html')

if __name__ == '__main__':
    app.run(port=8080)
