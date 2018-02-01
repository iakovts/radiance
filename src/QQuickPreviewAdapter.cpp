#include "QQuickPreviewAdapter.h"
#include "Model.h"
#include <memory>

QQuickPreviewAdapter::QQuickPreviewAdapter(bool hasPreview)
    : m_model(nullptr)
    , m_hasPreview(hasPreview)
    , m_previewSize(QSize(300, 300))
    , m_previewWindow(nullptr) {

    if (m_hasPreview) {
        m_previewChain = QSharedPointer<Chain>(new Chain(m_previewSize));
    }
}

QQuickPreviewAdapter::~QQuickPreviewAdapter() {
    if (m_model != nullptr && m_hasPreview) {
        m_model->removeChain(m_previewChain);
    }
}

Model *QQuickPreviewAdapter::model() {
    Q_ASSERT(QThread::currentThread() == thread());
    return m_model;
}

void QQuickPreviewAdapter::setModel(Model *model) {
    Q_ASSERT(QThread::currentThread() == thread());
    if (m_model != model) {
        if (m_model != nullptr && m_hasPreview) {
            m_model->removeChain(m_previewChain);
        }
        m_model = model;
        if (m_model != nullptr && m_hasPreview) {
            m_model->addChain(m_previewChain);
        }
        emit modelChanged(model);
    }
}

QSize QQuickPreviewAdapter::previewSize() {
    Q_ASSERT(m_hasPreview);
    QMutexLocker locker(&m_previewLock);
    return m_previewSize;
}

void QQuickPreviewAdapter::setPreviewSize(QSize size) {
    Q_ASSERT(m_hasPreview);
    Q_ASSERT(QThread::currentThread() == thread());
    if (size != m_previewSize) {
        {
            QMutexLocker locker(&m_previewLock);
            m_previewSize = size;
            QSharedPointer<Chain> previewChain(new Chain(size));
            if (m_model != nullptr) {
                m_model->removeChain(m_previewChain);
                m_model->addChain(previewChain);
            }
            m_previewChain = previewChain;
        }
        emit previewSizeChanged(size);
    }
}

QQuickWindow *QQuickPreviewAdapter::previewWindow() {
    return m_previewWindow;
}

void QQuickPreviewAdapter::setPreviewWindow(QQuickWindow *window) {
    Q_ASSERT(m_hasPreview);
    Q_ASSERT(QThread::currentThread() == thread());
    {
        QMutexLocker locker(&m_previewLock);
        if (m_previewWindow )
            disconnect(m_previewWindow, &QQuickWindow::beforeSynchronizing, this, &QQuickPreviewAdapter::onBeforeSynchronizing);
        m_previewWindow = window;
        if (m_previewWindow )
            connect(m_previewWindow, &QQuickWindow::beforeSynchronizing, this, &QQuickPreviewAdapter::onBeforeSynchronizing, Qt::DirectConnection);
    }
    emit previewWindowChanged(window);
}

void QQuickPreviewAdapter::onBeforeSynchronizing() {
    Q_ASSERT(m_hasPreview);
    auto modelCopy = m_model->createCopyForRendering(m_previewChain);
    m_lastPreviewRender = modelCopy.render(m_previewChain);
//    m_model->copyBackRenderStates(m_previewChain, &modelCopy);
}

GLuint QQuickPreviewAdapter::previewTexture(int videoNodeId) {
    Q_ASSERT(m_hasPreview);
    return m_lastPreviewRender.value(videoNodeId, 0);
}

//void QQuickPreviewAdapter::onRenderRequested(Output *output) {
//    auto name = output->name();
//    auto chain = output->chain();
//    auto modelCopy = m_model->createCopyForRendering(chain);
//    auto vnId = modelCopy.outputs.value(name, 0);
//    GLuint textureId = 0;
//    if (vnId != 0) { // Don't bother rendering this chain
//        // if it is not connected
//        auto result = modelCopy.render(chain);
//        textureId = result.value(vnId, 0);
//    }
//    output->renderReady(textureId);
//}
