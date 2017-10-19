#pragma once
#include <QQuickItem>
#include "Model.h"
#include "Controls.h"

struct Child {
    VideoNode *videoNode;
    QSharedPointer<QQuickItem> item;
    QVector<int> inputHeights;
};

class View : public QQuickItem {
    Q_OBJECT
    Q_PROPERTY(Model *model READ model WRITE setModel NOTIFY modelChanged)
    Q_PROPERTY(QVariantMap delegates READ qml_delegates WRITE qml_setDelegates NOTIFY qml_delegatesChanged)
    Q_PROPERTY(Controls *controls READ controls CONSTANT)

public:
    View();
    ~View() override;

    Model *model();
    void setModel(Model *model);
    QMap<QString, QString> delegates();
    void setDelegates(QMap<QString, QString> delegates);
    QVariantMap qml_delegates();
    void qml_setDelegates(QVariantMap delegates);

public slots:
    void onGraphChanged();

    // Selection
    void select(QVariantList tiles);
    void addToSelection(QVariantList tiles);
    void removeFromSelection(QVariantList tiles);
    void toggleSelection(QVariantList tiles);
    void ensureSelected(QQuickItem *tile);
    QVariantList selection();

    // Finds the connected components of the selection
    // Each connected component will have zero or more inputs and one output
    // (though possibly multiple output edges.)
    // This is useful because it may be treated as a single tile.
    // Returns a list of objects with three keys:
    // * tiles = A QVariantList of tiles contained within the connected component
    // * vertices = A QVariantList of vertices (VideoNodes) contained within the connected component
    // * edges = A QVariantList of edges contained within the connected component
    // * inputEdges = A QVariantList of input edges to the connected component (ordered)
    // * outputEdges = A QVariantList of output edges from the connected component (unordered)
    // * inputPorts = A QVariantList of QVariantMaps of {vertex, input}
    // * outputNode = The output VideoNode
    QVariantList selectedConnectedComponents();

    // Finds all tiles in between tile1 and tile2
    // Returns a QVariantList of tiles
    QVariantList tilesBetween(QQuickItem *tile1, QQuickItem *tile2);

    // Returns the tile for the given VideoNode instance
    QVariant tileForVideoNode(VideoNode *videoNode);

    // The tile that has focus,
    // or nullptr if no tile has focus
    QQuickItem *focusedChild();

    // Controls attached property
    // (for hooking up to MIDI)
    Controls *controls();

protected:
    Model *m_model;
    QMap<QString, QString> m_delegates;
    QList<Child> m_children;
    QList<QSharedPointer<QQuickItem>> m_dropAreas;
    Controls *m_controls;
    void rebuild();
    Child newChild(VideoNode *videoNode);
    QSet<QQuickItem *> m_selection;
    void selectionChanged();

protected slots:
    void onControlChangedAbs(int bank, Controls::Control control, qreal value);
    void onControlChangedRel(int bank, Controls::Control control, qreal value);

private:
    QSharedPointer<QQuickItem> createDropArea();

signals:
    void modelChanged(Model *model);
    void qml_delegatesChanged(QVariantMap delegates);
    void delegatesChanged(QMap<QString, QString> delegates);
};
