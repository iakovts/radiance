#include "OutputWindow.h"

#include <QScreen>
#include <QGuiApplication>

// OutputWindow

OutputWindow::OutputWindow(OutputNode *videoNode)
    : m_screenName("")
    , m_found(false)
    , m_videoNode(videoNode) {
    connect(this, &QWindow::screenChanged, this, &OutputWindow::onScreenChanged);

    setFlags(Qt::Dialog);
    setWindowState(Qt::WindowFullScreen);
    putOnScreen();
    connect(this, &QWindow::screenChanged, this, &OutputWindow::putOnScreen);

    reload();
    connect(&m_reloader, &QTimer::timeout, this, &OutputWindow::reload);
    m_reloader.setInterval(1000); // Reload screens every 1000 ms
    m_reloader.start();
}

OutputWindow::~OutputWindow() {
}

void OutputWindow::putOnScreen() {
    setGeometry(screen()->geometry());
}

QString OutputWindow::screenName() {
    return m_screenName;
}

void OutputWindow::onScreenChanged(QScreen *screen) {
    reload();
}

void OutputWindow::setScreenName(QString screenName) {
    if (screenName != m_screenName) {
        m_screenName = screenName;
        emit screenNameChanged(m_screenName);
        reload();
    }
}

void OutputWindow::reload() {
    auto screens = QGuiApplication::screens();

    bool found = false;

    foreach(QScreen *testScreen, screens) {
        if (testScreen->name() == m_screenName) {
            if (screen() != testScreen) {
                setScreen(testScreen);
            }
            found = true;
        }
    }

    if (found != m_found) {
        m_found = found;
        emit foundChanged(found);
    }
}

bool OutputWindow::found() {
    return m_found;
}

void OutputWindow::initializeGL()
{
    auto vertexString = QString{
        "#version 150\n"
        "const vec2 varray[4] = vec2[](vec2(1., 1.),vec2(1., -1.),vec2(-1., 1.),vec2(-1., -1.));\n"
        "out vec2 uv;\n"
        "void main() {"
        "    vec2 vertex = varray[gl_VertexID];\n"
        "    gl_Position = vec4(vertex,0.,1.);\n"
        "    uv = 0.5 * (vertex + 1.);\n"
        "}"};

    auto fragmentString = QString{
        "#version 150\n"
        "uniform sampler2D iTexture;\n"
        "varying vec2 uv;\n"
        "out vec4 fragColor;\n"
        "void main() {\n"
        "   fragColor = vec4(texture(iTexture, uv).rgb, 1.);\n"
        "}"};;

    m_program = new QOpenGLShaderProgram(this);
    m_program->addShaderFromSourceCode(QOpenGLShader::Vertex, vertexString);
    m_program->addShaderFromSourceCode(QOpenGLShader::Fragment, fragmentString);
    m_program->link();
}

void OutputWindow::resizeGL(int w, int h) {
}

void OutputWindow::paintGL() {
    GLuint texture = m_videoNode->render();
    auto dpr = devicePixelRatio();
    glViewport(0, 0, width() * dpr, height() * dpr);
    glClearColor(0, 0, 0, 0);
    glDisable(GL_DEPTH_TEST);
    glDisable(GL_BLEND);
    glClear(GL_COLOR_BUFFER_BIT);
    m_program->bind();
    glActiveTexture(GL_TEXTURE0);
    glBindTexture(GL_TEXTURE_2D, texture);
    m_videoNode->chain()->vao().bind();
    glDrawArrays(GL_TRIANGLE_STRIP, 0, 4);
    m_videoNode->chain()->vao().release();
    m_program->release();
    update();
}
